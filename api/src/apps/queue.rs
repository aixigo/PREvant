use crate::apps::repository::AppPostgresRepository;
use crate::apps::{Apps, AppsError};
use crate::models::{
    App, AppName, AppStatusChangeId, AppTask, MergedAppTask, Owner, ServiceConfig,
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
                let merged = AppTask::merge_tasks(tasks);

                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "Processing task {} for {}.",
                        merged.task_to_work_on.status_id(),
                        merged.task_to_work_on.app_name()
                    );
                }

                let result = match &merged.task_to_work_on {
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

                        apps
                            .delete_app_partially(app_name, infrastructure_payload_to_back_up)
                            .await
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

                        apps
                            .restore_app_partially(app_name, infrastructure_payload_to_restore)
                            .await
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

                        apps
                            .create_or_update(
                                app_name,
                                replicate_from.clone(),
                                service_configs,
                                owners.clone(),
                                user_defined_parameters.clone(),
                            )
                            .await
                    }
                    AppTask::Delete { app_name, .. } => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("Deleting app {app_name}.");
                        }
                        apps.delete_app(app_name).await
                    }
                };

                (merged, result)
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
                if matches!(
                    task,
                    AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure { .. }
                        | AppTask::RestoreOnInfrastructureAndDeleteFromBackup { .. }
                ) {
                    anyhow::bail!(
                        "Backup or restore is not supported by in-memory app queue processing."
                    );
                }

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

    async fn execute_tasks<F, Fut>(&self, executor: F) -> Result<()>
    where
        F: FnOnce(Vec<AppTask>) -> Fut,
        Fut: Future<Output = (MergedAppTask, std::result::Result<App, AppsError>)>,
    {
        match self {
            AppTaskQueueDB::InMemory(mutex) => {
                let tasks = {
                    let mut queue = mutex.lock().await;

                    let mut iter = queue.iter_mut();

                    let mut tasks = Vec::new();
                    let mut app_name = None;
                    for (task, s) in iter.by_ref() {
                        app_name = match (&s, app_name.take()) {
                            (AppTaskStatus::New, current_app_name) => {
                                if current_app_name.is_some()
                                    && Some(task.app_name()) != current_app_name
                                {
                                    break;
                                }
                                *s = AppTaskStatus::InProcess;
                                tasks.push(task.clone());
                                Some(task.app_name())
                            }
                            (AppTaskStatus::InProcess, None) => {
                                log::warn!("Trying to find tasks to be process but there is currently {} in process", task.app_name());
                                tasks.clear();
                                break;
                            }
                            (AppTaskStatus::InProcess, Some(app_name)) => {
                                log::error!(
                                    "The interior queue status seem to be messed up while searching for tasks for {app_name} we found a in-process task for {}",
                                    task.app_name()
                                );
                                tasks.clear();
                                break;
                            }
                            (AppTaskStatus::Done(_), _) => continue,
                        }
                    }

                    tasks
                };

                if tasks.is_empty() {
                    return Ok(());
                }

                let (merged, result) = executor(tasks).await;
                let done_timestamp = Utc::now();

                let mut queue = mutex.lock().await;
                let task_worked_on = merged.task_to_work_on;
                let status_id = task_worked_on.status_id();

                let Some(task) = queue
                    .iter_mut()
                    .find(|(task, _)| task.status_id() == status_id)
                else {
                    anyhow::bail!("Cannot update {status_id} in queue which should be present");
                };

                task.1 = AppTaskStatus::Done((done_timestamp, result.clone()));

                for (task, s) in queue.iter_mut() {
                    if merged.tasks_to_be_marked_as_done.contains(task.status_id()) {
                        *s = AppTaskStatus::Done((done_timestamp, result.clone()));
                    }
                    if merged.tasks_to_stay_untouched.contains(task.status_id()) {
                        *s = AppTaskStatus::New;
                    }
                }

                Ok(())
            }
            AppTaskQueueDB::DB(db) => db.lock_queued_tasks_and_perform_executor(executor).await,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::DatabasePool, models::AppName, sc};
    use rstest::rstest;
    use std::collections::HashSet;
    use testcontainers_modules::{
        postgres::{self},
        testcontainers::{runners::AsyncRunner, ContainerAsync},
    };

    enum TestQueue {
        InMemory(Box<AppTaskQueueDB>),
        DB(Box<(ContainerAsync<postgres::Postgres>, AppTaskQueueDB)>),
    }

    impl TestQueue {
        fn inmemory() -> Self {
            Self::InMemory(Box::new(AppTaskQueueDB::inmemory()))
        }

        async fn postgres_queue() -> Self {
            use sqlx::postgres::PgConnectOptions;
            let postgres_instance = postgres::Postgres::default().start().await.unwrap();

            let connection = PgConnectOptions::new_without_pgpass()
                .application_name("PREvant")
                .host(&postgres_instance.get_host().await.unwrap().to_string())
                .port(postgres_instance.get_host_port_ipv4(5432).await.unwrap())
                .username("postgres")
                .password("postgres");

            let pool = DatabasePool::connect_with_exponential_backoff(connection)
                .await
                .unwrap();
            sqlx::migrate!().run(&pool).await.unwrap();

            Self::DB(Box::new((
                postgres_instance,
                AppTaskQueueDB::db(AppPostgresRepository::new(pool)),
            )))
        }

        async fn enqueue_task(&self, task: AppTask) -> Result<()> {
            match self {
                TestQueue::InMemory(queue) => queue.enqueue_task(task).await,
                TestQueue::DB(b) => b.1.enqueue_task(task).await,
            }
        }

        async fn execute_tasks<F, Fut>(&self, executor: F) -> Result<()>
        where
            F: FnOnce(Vec<AppTask>) -> Fut,
            Fut: Future<Output = (MergedAppTask, std::result::Result<App, AppsError>)>,
        {
            match self {
                TestQueue::InMemory(queue) => queue.execute_tasks(executor).await,
                TestQueue::DB(b) => b.1.execute_tasks(executor).await,
            }
        }

        async fn peek_result(
            &self,
            status_id: &AppStatusChangeId,
        ) -> Option<std::result::Result<App, AppsError>> {
            match self {
                TestQueue::InMemory(queue) => queue.peek_result(status_id).await,
                TestQueue::DB(b) => b.1.peek_result(status_id).await,
            }
        }

        async fn clean_up_done_tasks(&self, older_than: DateTime<Utc>) -> Result<usize> {
            match self {
                TestQueue::InMemory(queue) => queue.clean_up_done_tasks(older_than).await,
                TestQueue::DB(b) => b.1.clean_up_done_tasks(older_than).await,
            }
        }
    }

    fn simulate_result(tasks: Vec<AppTask>) -> (MergedAppTask, Result<App, AppsError>) {
        (
            AppTask::merge_tasks(tasks),
            Ok(App::new(
                vec![crate::models::Service {
                    id: String::from("nginx-1234"),
                    state: crate::models::State {
                        status: crate::models::ServiceStatus::Paused,
                        started_at: None,
                    },
                    config: sc!("nginx"),
                }],
                HashSet::new(),
                None,
            )),
        )
    }

    #[rstest]
    #[case::inmemory(async { TestQueue::inmemory() })]
    #[tokio::test]
    async fn inmemory_queue_cannot_handle_back_up_and_restore(
        #[future]
        #[case]
        queue: TestQueue,
    ) {
        let queue = queue.await;

        let err = queue
            .enqueue_task(AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                status_id: AppStatusChangeId::new(),
                app_name: AppName::master(),
                infrastructure_payload_to_back_up: vec![serde_json::json!({})],
            })
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Backup or restore is not supported by in-memory app queue processing."
        );

        let err = queue
            .enqueue_task(AppTask::RestoreOnInfrastructureAndDeleteFromBackup {
                status_id: AppStatusChangeId::new(),
                app_name: AppName::master(),
                infrastructure_payload_to_restore: vec![serde_json::json!({})],
            })
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Backup or restore is not supported by in-memory app queue processing."
        );
    }

    #[rstest]
    #[case::inmemory(async { TestQueue::inmemory() })]
    #[case::postgres(async { TestQueue::postgres_queue().await })]
    #[tokio::test]
    async fn enqueue_nothing_and_execute_single_task(
        #[future]
        #[case]
        queue: TestQueue,
    ) {
        let queue = queue.await;

        let result = queue
            .execute_tasks(async |_tasks| unreachable!("Empty queue shouldn't trigger this code"))
            .await;

        assert!(result.is_ok())
    }

    #[rstest]
    #[case::inmemory(async { TestQueue::inmemory() })]
    #[case::postgres(async { TestQueue::postgres_queue().await })]
    #[tokio::test]
    async fn enqueue_and_execute_single_task(
        #[future]
        #[case]
        queue: TestQueue,
    ) {
        let queue = queue.await;

        let status_id = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        queue
            .execute_tasks(async |tasks| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.peek_result(&status_id).await;
        assert!(matches!(result, Some(Ok(_))));

        let cleaned = queue.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 1);
    }

    #[rstest]
    #[case::inmemory(async { TestQueue::inmemory() })]
    #[case::postgres(async { TestQueue::postgres_queue().await })]
    #[tokio::test]
    async fn enqueue_and_execute_merged_task(
        #[future]
        #[case]
        queue: TestQueue,
    ) {
        let queue = queue.await;

        let status_id_1 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();
        let status_id_2 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        queue
            .execute_tasks(async |tasks| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.peek_result(&status_id_2).await;
        assert!(matches!(result, Some(Ok(_))));

        let cleaned = queue.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 2);
    }

    #[rstest]
    #[case::inmemory(async { TestQueue::inmemory() })]
    #[case::postgres(async { TestQueue::postgres_queue().await })]
    #[tokio::test]
    async fn enqueue_and_handle_one_app_at_the_time(
        #[future]
        #[case]
        queue: TestQueue,
    ) {
        use std::str::FromStr;

        let queue = queue.await;

        let status_id_1 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let status_id_2 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::from_str("other").unwrap(),
            })
            .await
            .unwrap();

        queue
            .execute_tasks(async |tasks| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.peek_result(&status_id_2).await;
        assert!(result.is_none());
    }

    #[rstest]
    #[case::inmemory(async { TestQueue::inmemory() })]
    #[case::postgres(async { TestQueue::postgres_queue().await })]
    #[tokio::test]
    async fn enqueue_and_handle_one_app_after_the_other(
        #[future]
        #[case]
        queue: TestQueue,
    ) {
        use std::str::FromStr;

        let queue = queue.await;

        let status_id_1 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let status_id_2 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::from_str("other").unwrap(),
            })
            .await
            .unwrap();

        queue
            .execute_tasks(async |tasks| simulate_result(tasks))
            .await
            .unwrap();
        queue
            .execute_tasks(async |tasks| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.peek_result(&status_id_2).await;
        assert!(matches!(result, Some(Ok(_))));
    }

    #[rstest]
    #[case::postgres(async { TestQueue::postgres_queue().await })]
    #[tokio::test]
    async fn enqueue_and_handle_none_mergeable_tasks(
        #[future]
        #[case]
        queue: TestQueue,
    ) {
        let _ = env_logger::builder().is_test(true).try_init();

        let queue = queue.await;

        let status_id_1 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let status_id_2 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                status_id: status_id_2,
                app_name: AppName::master(),
                infrastructure_payload_to_back_up: vec![serde_json::json!({
                    "string-key": "test",
                    "array-key": [1, 2, 3]
                })],
            })
            .await
            .unwrap();

        queue
            .execute_tasks(async |tasks| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.peek_result(&status_id_2).await;
        assert!(result.is_none());
    }
}
