use crate::apps::repository::AppPostgresRepository;
use crate::apps::{Apps, AppsError};
use crate::models::{App, AppName, AppStatusChangeId, AppTask, Owner, ServiceConfig};
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
        let db = match rocket.state::<Option<AppPostgresRepository>>() {
            Some(Some(repository)) => AppTaskQueueDB::db(repository.clone()),
            _ => AppTaskQueueDB::inmemory(),
        };

        let producer = AppTaskQueueProducer {
            db: Arc::new(db),
            notify: Arc::new(Notify::new()),
        };

        Ok(rocket.manage(producer))
    }

    async fn on_liftoff(&self, rocket: &Rocket<Orbit>) {
        let apps = rocket.state::<Apps>().unwrap();
        let producer = rocket.state::<AppTaskQueueProducer>().unwrap();
        let consumer = AppTaskQueueConsumer {
            db: producer.db.clone(),
            notify: producer.notify.clone(),
        };
        let mut shutdown = rocket.shutdown();

        let apps = apps.clone();
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
        owner: Option<Owner>,
        user_defined_parameters: Option<serde_json::Value>,
    ) -> Result<AppStatusChangeId> {
        let status_id = AppStatusChangeId::new();
        self.db
            .enqueue_task(AppTask::CreateOrUpdate {
                app_name,
                status_id,
                replicate_from,
                service_configs,
                owners: owner.into_iter().collect(),
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

    pub async fn enqueue_backup_task(
        &self,
        app_name: AppName,
        infrastructure_payload: Vec<serde_json::Value>,
    ) -> Result<AppStatusChangeId> {
        let status_id = AppStatusChangeId::new();
        self.db
            .enqueue_task(AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                status_id,
                app_name,
                infrastructure_payload_to_back_up: infrastructure_payload,
            })
            .await?;

        if log::log_enabled!(log::Level::Debug) {
            log::debug!("Notify about new backup task: {status_id}.");
        }
        self.notify.notify_one();

        Ok(status_id)
    }

    pub async fn enqueue_restore_task(
        &self,
        app_name: AppName,
        infrastructure_payload: Vec<serde_json::Value>,
    ) -> Result<AppStatusChangeId> {
        let status_id = AppStatusChangeId::new();
        self.db
            .enqueue_task(AppTask::RestoreOnInfrastructureAndDeleteFromBackup {
                status_id,
                app_name,
                infrastructure_payload_to_restore: infrastructure_payload,
            })
            .await?;

        if log::log_enabled!(log::Level::Debug) {
            log::debug!("Notify about new restore task: {status_id}.");
        }
        self.notify.notify_one();

        Ok(status_id)
    }

    pub async fn try_wait_for_task(
        &self,
        status_id: &AppStatusChangeId,
        wait_timeout: Duration,
    ) -> Option<std::result::Result<App, AppsError>> {
        let interval = Duration::from_secs(2);

        let mut interval_timer = tokio::time::interval(interval);
        let start_time = tokio::time::Instant::now();

        loop {
            tokio::select! {
                _ = interval_timer.tick() => {
                    match timeout(wait_timeout, self.db.peek_result(status_id)).await {
                        Ok(Some(result)) => return Some(result),
                        Ok(None) => continue,
                        Err(err) => {
                            log::debug!("Did not receive result within {} sec: {err}", wait_timeout.as_secs());
                            break;
                        }
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
    pub async fn process_next_task(&self, apps: &Apps) -> Result<()> {
        tokio::select! {
            _ = self.notify.notified() => {
                log::debug!("Got notified by another thread to check for new items in the queue.");
            }
            _ = sleep(Duration::from_secs(30)) => {
                log::debug!("Regular task check.");
            }
        }

        self.db
            .execute_tasks(async |tasks| {
                let Some(task) = tasks.into_iter().reduce(|acc, e| acc.merge_with(e)) else {
                    panic!("tasks must not be empty");
                };

                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "Processing task {} for {}.",
                        task.status_id(),
                        task.app_name()
                    );
                }
                match &task {
                    AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                        app_name,
                        infrastructure_payload_to_back_up,
                        ..
                    } => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!(
                                "Dropping infrastructure objects for {app_name} due to back up."
                            );
                        }

                        let result = apps
                            .delete_app_partially(app_name, infrastructure_payload_to_back_up)
                            .await;
                        (task, result)
                    }
                    AppTask::RestoreOnInfrastructureAndDeleteFromBackup {
                        app_name,
                        infrastructure_payload_to_restore,
                        ..
                    } => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!(
                                "Restoring infrastructure objects for {app_name} due to restore task."
                            );
                        }

                        let result = apps
                            .restore_app_partially(app_name, infrastructure_payload_to_restore)
                            .await;
                        (task, result)
                    },
                    AppTask::CreateOrUpdate {
                        app_name,
                        replicate_from,
                        service_configs,
                        owners,
                        user_defined_parameters,
                        ..
                    } => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("Creating or updating app {app_name}.");
                        }

                        let result = apps
                            .create_or_update(
                                app_name,
                                replicate_from.clone(),
                                service_configs,
                                owners.clone(),
                                user_defined_parameters.clone(),
                            )
                            .await;
                        (task, result)
                    }
                    AppTask::Delete { app_name, .. } => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("Deleting app {app_name}.");
                        }
                        let result = apps.delete_app(app_name).await;
                        (task, result)
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

enum AppTaskStatus {
    New,
    InProcess,
    Done((DateTime<Utc>, std::result::Result<App, AppsError>)),
}

enum AppTaskQueueDB {
    InMemory(Mutex<VecDeque<(AppTask, AppTaskStatus)>>),
    DB(AppPostgresRepository),
}

impl AppTaskQueueDB {
    fn inmemory() -> Self {
        Self::InMemory(Mutex::new(VecDeque::new()))
    }

    fn db(repository: AppPostgresRepository) -> Self {
        Self::DB(repository)
    }

    pub async fn enqueue_task(&self, task: AppTask) -> Result<()> {
        match self {
            AppTaskQueueDB::InMemory(mutex) => {
                // TODO: fail for in memory backup
                let mut queue = mutex.lock().await;
                queue.push_back((task, AppTaskStatus::New));
            }
            AppTaskQueueDB::DB(db) => {
                db.enqueue_task(task).await?;
            }
        }

        Ok(())
    }

    async fn peek_result(
        &self,
        status_id: &AppStatusChangeId,
    ) -> Option<std::result::Result<App, AppsError>> {
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
                None
            }
            AppTaskQueueDB::DB(db) => db.peek_result(status_id).await,
        }
    }

    async fn execute_tasks<F, Fut>(&self, f: F) -> Result<()>
    where
        F: FnOnce(Vec<AppTask>) -> Fut,
        Fut: Future<Output = (AppTask, std::result::Result<App, AppsError>)>,
    {
        match self {
            AppTaskQueueDB::InMemory(mutex) => {
                let task = {
                    let mut queue = mutex.lock().await;

                    // TODO: we should process multiple tasks here too
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
                let (task_worked_on, result) = f(vec![task]).await;

                let mut queue = mutex.lock().await;
                let Some(task) = queue
                    .iter_mut()
                    .find(|(task, _)| task.status_id() == &status_id)
                else {
                    anyhow::bail!("Cannot update {status_id} in queue which should be present");
                };

                assert!(task_worked_on.status_id() == task.0.status_id());
                task.1 = AppTaskStatus::Done((Utc::now(), result));

                Ok(())
            }
            AppTaskQueueDB::DB(db) => db.update_queued_tasks_with_executor_result(f).await,
        }
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
            AppTaskQueueDB::DB(db) => db.clean_up_done_tasks(older_than).await,
        }
    }
}
