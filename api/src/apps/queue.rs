use super::{AppsService, AppsServiceError};
use crate::{
    auth::User,
    models::{App, AppName, AppStatusChangeId, ServiceConfig},
};
use anyhow::Result;
use chrono::{DateTime, TimeDelta, Utc};
use rocket::{
    fairing::{Fairing, Info, Kind},
    Build, Orbit, Rocket,
};
use std::{collections::VecDeque, future::Future, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, Notify},
    time::{sleep, sleep_until, timeout},
};

pub struct AppProcessingQueue {}

impl AppProcessingQueue {
    pub fn fairing() -> Self {
        Self {}
    }
}

#[rocket::async_trait]
impl Fairing for AppProcessingQueue {
    fn info(&self) -> Info {
        Info {
            name: "app-queue",
            kind: Kind::Ignite | Kind::Liftoff,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<Build>) -> rocket::fairing::Result {
        let producer = AppTaskQueueProducer {
            db: Arc::new(AppTaskQueueDB::inmemory()),
            notify: Arc::new(Notify::new()),
        };

        Ok(rocket.manage(producer))
    }

    async fn on_liftoff(&self, rocket: &Rocket<Orbit>) {
        let apps = rocket.state::<Arc<AppsService>>().unwrap().clone();
        let producer = rocket.state::<AppTaskQueueProducer>().unwrap();
        let consumer = AppTaskQueueConsumer {
            db: producer.db.clone(),
            notify: producer.notify.clone(),
        };
        let mut shutdown = rocket.shutdown();

        rocket::tokio::spawn(async move {
            loop {
                tokio::select! {
                    res = consumer.process_next_task(&apps) => {
                        if let Err(err) = res {
                            log::error!("Cannot process task: {err}");
                        }
                    }
                    _ = &mut shutdown => {
                        log::info!("Shutting down queue processing");
                        break;
                    }
                };

                match consumer.clean_up_done_tasks().await {
                    Ok(number_of_deleted_tasks) if number_of_deleted_tasks > 0 => {
                        log::debug!("Deleted {number_of_deleted_tasks} done tasks");
                    }
                    Err(err) => {
                        log::error!("Cannot cleanup done task: {err}");
                    }
                    _ => {}
                }
            }
        });
    }
}

pub struct AppTaskQueueProducer {
    db: Arc<AppTaskQueueDB>,
    notify: Arc<Notify>,
}
impl AppTaskQueueProducer {
    pub async fn enqueue_create_or_update_task(
        &self,
        app_name: AppName,
        replicate_from: Option<AppName>,
        service_configs: Vec<ServiceConfig>,
        user: User,
        user_defined_parameters: Option<serde_json::Value>,
    ) -> Result<AppStatusChangeId> {
        let status_id = AppStatusChangeId::new();
        self.db
            .enqueue_task(AppTask::CreateOrUpdate {
                app_name,
                status_id,
                replicate_from,
                service_configs,
                user,
                user_defined_parameters,
            })
            .await?;

        if log::log_enabled!(log::Level::Debug) {
            log::debug!("Notify about new create or update task: {status_id}.");
        }
        self.notify.notify_one();

        Ok(status_id)
    }

    pub async fn enqueue_delete_task(&self, app_name: AppName) -> Result<AppStatusChangeId> {
        let status_id = AppStatusChangeId::new();
        self.db
            .enqueue_task(AppTask::Delete {
                app_name,
                status_id,
            })
            .await?;

        if log::log_enabled!(log::Level::Debug) {
            log::debug!("Notify about new delete task: {status_id}.");
        }
        self.notify.notify_one();

        Ok(status_id)
    }

    pub async fn try_wait_for_task(
        &self,
        status_id: &AppStatusChangeId,
        wait_timeout: Duration,
    ) -> Option<std::result::Result<App, AppsServiceError>> {
        let interval = Duration::from_secs(2);

        let mut interval_timer = tokio::time::interval(interval);
        let start_time = tokio::time::Instant::now();

        loop {
            tokio::select! {
                _ = interval_timer.tick() => {
                    match timeout(wait_timeout, self.db.peek_result(status_id)).await {
                        Ok(Some(result)) => return Some(result),
                        // TODO: correct so far?
                        Ok(None) => continue,
                        Err(_) => todo!()
                    }
                }
                _ = sleep_until(start_time + wait_timeout) => {
                    log::debug!("Timeout reached, stopping querying the queue");
                    break;
                }
            }
        }

        None
    }
}

struct AppTaskQueueConsumer {
    db: Arc<AppTaskQueueDB>,
    notify: Arc<Notify>,
}

impl AppTaskQueueConsumer {
    pub async fn process_next_task(&self, apps: &AppsService) -> Result<()> {
        tokio::select! {
            _ = self.notify.notified() => {
                log::debug!("Got notified by another thread to check for new items in the queue.");
            }
            _ = sleep(Duration::from_secs(30)) => {
                log::debug!("Regular task check.");
            }
        }

        self.db
            .execute_task(async |task| {
                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "Processing task {} for {}.",
                        task.status_id(),
                        task.app_name()
                    );
                }
                match task {
                    AppTask::CreateOrUpdate {
                        app_name,
                        replicate_from,
                        service_configs,
                        user,
                        user_defined_parameters,
                        ..
                    } => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("Creating or updating app {app_name}.");
                        }
                        apps.create_or_update(
                            &app_name,
                            replicate_from,
                            &service_configs,
                            user,
                            user_defined_parameters,
                        )
                        .await
                    }
                    AppTask::Delete { app_name, .. } => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("Deleting app {app_name}.");
                        }
                        apps.delete_app(&app_name).await
                    }
                }
            })
            .await
    }

    pub async fn clean_up_done_tasks(&self) -> Result<usize> {
        let an_hour_ago = Utc::now() - TimeDelta::hours(1);
        self.db.clean_up_done_tasks(an_hour_ago).await
    }
}

#[derive(Clone)]
enum AppTask {
    CreateOrUpdate {
        app_name: AppName,
        status_id: AppStatusChangeId,
        replicate_from: Option<AppName>,
        service_configs: Vec<ServiceConfig>,
        user: User,
        user_defined_parameters: Option<serde_json::Value>,
    },
    Delete {
        status_id: AppStatusChangeId,
        app_name: AppName,
    },
}
impl AppTask {
    fn app_name(&self) -> &AppName {
        match self {
            AppTask::CreateOrUpdate { app_name, .. } => app_name,
            AppTask::Delete { app_name, .. } => app_name,
        }
    }
    fn status_id(&self) -> &AppStatusChangeId {
        match self {
            AppTask::CreateOrUpdate { status_id, .. } => status_id,
            AppTask::Delete { status_id, .. } => status_id,
        }
    }
}
pub enum AppTaskStatus {
    New,
    InProcess,
    Done((DateTime<Utc>, std::result::Result<App, AppsServiceError>)),
}

enum AppTaskQueueDB {
    InMemory(Mutex<VecDeque<(AppTask, AppTaskStatus)>>),
}

impl AppTaskQueueDB {
    fn inmemory() -> Self {
        Self::InMemory(Mutex::new(VecDeque::new()))
    }

    pub async fn enqueue_task(&self, task: AppTask) -> Result<()> {
        match self {
            AppTaskQueueDB::InMemory(mutex) => {
                let mut queue = mutex.lock().await;
                queue.push_back((task, AppTaskStatus::New));
            }
        }

        Ok(())
    }

    async fn peek_result(
        &self,
        status_id: &AppStatusChangeId,
    ) -> Option<std::result::Result<App, AppsServiceError>> {
        log::debug!("Checking for results for {status_id}.");
        match self {
            AppTaskQueueDB::InMemory(mutex) => {
                let queue = mutex.lock().await;

                for (task, status) in queue.iter() {
                    if task.status_id() == status_id {
                        if let AppTaskStatus::Done((_, result)) = status {
                            return Some(result.clone());
                        }
                    }
                }
            }
        }
        None
    }

    async fn execute_task<F, Fut>(&self, f: F) -> Result<()>
    where
        F: FnOnce(AppTask) -> Fut,
        Fut: Future<Output = std::result::Result<App, AppsServiceError>>,
    {
        match self {
            AppTaskQueueDB::InMemory(mutex) => {
                let task = {
                    let mut queue = mutex.lock().await;

                    let Some(task) = queue
                        .iter_mut()
                        .find(|(_, s)| matches!(s, AppTaskStatus::New))
                    else {
                        return Ok(());
                    };

                    task.1 = AppTaskStatus::InProcess;

                    task.0.clone()
                };

                let status_id = *task.status_id();
                let result = f(task).await;

                let mut queue = mutex.lock().await;
                let Some(task) = queue
                    .iter_mut()
                    .find(|(task, _)| task.status_id() == &status_id)
                else {
                    anyhow::bail!("Cannot update {status_id} in queue which should be present");
                };

                task.1 = AppTaskStatus::Done((Utc::now(), result));
            }
        };

        Ok(())
    }

    async fn clean_up_done_tasks(&self, older_than: DateTime<Utc>) -> Result<usize> {
        match self {
            AppTaskQueueDB::InMemory(mutex) => {
                let mut queue = mutex.lock().await;

                let before = queue.len();
                queue.retain(
                    |(_, status)| !matches!(status, AppTaskStatus::Done((timestamp, _)) if timestamp < &older_than),
                );

                Ok(before - queue.len())
            }
        }
    }
}
