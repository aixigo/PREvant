use crate::apps::{
    AppTask, Apps, AppsError, CreateOrUpdateParams, MergedAppTask, queue::inmemory::InMemoryQueue,
    repository::AppPostgresRepository,
};
use crate::infrastructure::{Infrastructure, InfrastructureProxy};
use crate::models::AppStatusChangeId;
use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use domain::{
    AppName, Owner,
    app_blueprints::{ServiceConfig, UserDefinedParameters},
    app_instance::App,
};
use rocket::{
    Build, Orbit, Rocket,
    fairing::{Fairing, Info, Kind},
};
use std::collections::{BTreeSet, HashSet};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{
    sync::Notify,
    time::{sleep, sleep_until, timeout},
};

mod db;
mod inmemory;

#[cfg(not(test))]
const TASK_PROCESSING_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(10);
#[cfg(test)]
pub const TASK_PROCESSING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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

#[derive(Clone)]
pub struct AppTaskQueueProducer {
    db: Arc<AppTaskQueueDB>,
    notify: Arc<Notify>,
}
impl AppTaskQueueProducer {
    /// Creates a producer that has no consuming worker. For testing purposes only.
    #[cfg(test)]
    pub fn no_op() -> Self {
        Self {
            db: Arc::new(AppTaskQueueDB::inmemory()),
            notify: Arc::new(Notify::new()),
        }
    }

    pub async fn enqueue_create_or_update_task(
        &self,
        app_name: AppName,
        replicate_from: Option<AppName>,
        service_configs: Vec<ServiceConfig>,
        owner: Option<Owner>,
        user_defined_parameters: Option<UserDefinedParameters>,
    ) -> Result<AppStatusChangeId> {
        let status_id = AppStatusChangeId::new();
        self.db
            .enqueue_task(AppTask::CreateOrUpdate {
                app_name,
                status_id,
                replicate_from,
                service_configs,
                owners: owner.into_iter().collect(),
                user_defined_parameters: user_defined_parameters.map(|udp| udp.into_value()),
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
        infrastructure_payload: <InfrastructureProxy as crate::infrastructure::Infrastructure>::BackupPayload,
    ) -> Result<AppStatusChangeId> {
        let status_id = AppStatusChangeId::new();
        self.db
            .enqueue_task(AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                status_id,
                app_name,
                infrastructure_payload_to_back_up: serde_json::to_value(infrastructure_payload)
                    .context("Cannot convert infrastructure payload to JSON")?,
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
        infrastructure_payload: serde_json::Value,
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
            .execute_tasks(async |merged, backed_up_apps| {
                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "Processing task {} for {}.",
                        merged.task_to_work_on.status_id(),
                        merged.task_to_work_on.app_name()
                    );
                }

                match &merged.task_to_work_on {
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

                        match serde_json::from_value::<<InfrastructureProxy as Infrastructure>::BackupPayload>(infrastructure_payload_to_back_up.clone()) {
                            Ok(infrastructure_payload_to_back_up) => {
                                apps
                                    .delete_app_partially(app_name, infrastructure_payload_to_back_up)
                                    .await
                            }
                            Err(err) => {
                                log::error!("Cannot deserialize the infrastructure payload to be restored: {err}");
                                // TODO: there should be another error that also makes sure to clean
                                // up the back-up data int the repository. The method execute_tasks
                                // must have its own error type to deal with these cases in order to
                                // differentiate between retry and broken (never try again).
                                Err(AppsError::AppNotFound { app_name: app_name.clone() })
                            },
                        }
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

                        match serde_json::from_value::<<InfrastructureProxy as Infrastructure>::BackupPayload>(infrastructure_payload_to_restore.clone()) {
                            Ok(infrastructure_payload_to_restore) => {
                                apps
                                    .restore_app_partially(app_name, infrastructure_payload_to_restore)
                                    .await
                                    .and_then(|app| app.ok_or_else(|| AppsError::AppNotFound {
                                        app_name: app_name.clone()
                                    }))
                            }
                            Err(err) => {
                                log::error!("Cannot deserialize the infrastructure payload to be restored: {err}");
                                // TODO: there should be another error that also makes sure to clean
                                // up the back-up data int the repository. The method execute_tasks
                                // must have its own error type to deal with these cases in order to
                                // differentiate between retry and broken (never try again).
                                Err(AppsError::AppNotFound { app_name: app_name.clone() })
                            },
                        }

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
                            .create_or_update(CreateOrUpdateParams {
                                app_name: app_name.clone(),
                                replicate_from: replicate_from.clone(),
                                service_configs: service_configs.clone(),
                                owners: owners.clone(),
                                user_defined_parameters: user_defined_parameters.as_ref().map(|udp| unsafe {
                                    // SAFETY: the data has been validated before storing int the
                                    // queue, so it is safe to skip validation here.
                                    UserDefinedParameters::without_validation(udp.clone())
                                }),
                                backed_up_apps,
                            }).await
                    }
                    AppTask::Delete { app_name, .. } => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("Deleting app {app_name}.");
                        }
                        apps.delete_app(app_name).await
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

#[derive(Debug)]
pub struct ProcessableTasks {
    pub tasks: Vec<AppTask>,
    pub backed_up_tasks: HashSet<AppName>,
    phantom_data: std::marker::PhantomData<()>,
}

impl ProcessableTasks {
    pub fn new(
        tasks: Vec<AppTask>,
        backed_up_tasks: HashSet<AppName>,
    ) -> Result<Self, ProcessableTasksError> {
        let mut it = tasks.iter();
        let Some(app_name) = it.next().map(|task| task.app_name()) else {
            return Err(ProcessableTasksError::MustProvideAtLeastOneTask);
        };

        for task in it {
            if app_name != task.app_name() {
                return Err(ProcessableTasksError::AppNamesDoNotMatch {
                    app_names: tasks
                        .iter()
                        .map(|task| task.app_name().clone())
                        .collect::<BTreeSet<_>>(),
                });
            }
        }

        Ok(Self {
            tasks,
            backed_up_tasks,
            phantom_data: std::marker::PhantomData,
        })
    }
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum ProcessableTasksError {
    #[error("At least one task must be provided")]
    MustProvideAtLeastOneTask,
    #[error(
        "Tasks do not refer to the same applications: {}",
        app_names.iter()
            .map(|a| a.to_string())
            .reduce(|a, b| a + ", " + &b)
            .unwrap_or_default())
    ]
    AppNamesDoNotMatch { app_names: BTreeSet<AppName> },
}

#[async_trait::async_trait]
pub trait AppTaskQueue {
    type LockContext<'lc>: Send
    where
        Self: 'lc;
    type Error: std::error::Error;

    async fn enqueue_task(&self, task: AppTask) -> std::result::Result<(), Self::Error>;

    async fn peek_result(
        &self,
        status_id: &AppStatusChangeId,
    ) -> Option<std::result::Result<App, AppsError>>;

    /// Fetches all [`AppTask`]s that are processable in one batch and returns [`Self::LockContext`]
    /// so that follow-up operations can be performed atomically.
    async fn fetch_processable_tasks<'lc, 'a: 'lc>(
        &'a self,
    ) -> std::result::Result<Option<(ProcessableTasks, Self::LockContext<'lc>)>, Self::Error>;

    async fn store_result<'lc>(
        &self,
        lock_ctx: Self::LockContext<'lc>,
        result: std::result::Result<App, AppsError>,
        done_timestamp: DateTime<Utc>,
        merged: &MergedAppTask,
    ) -> std::result::Result<(), Self::Error>;

    async fn clean_up_done_tasks(
        &self,
        older_than: DateTime<Utc>,
    ) -> std::result::Result<usize, Self::Error>;
}

enum AppTaskQueueDB {
    InMemory(InMemoryQueue),
    DB(AppPostgresRepository),
}

impl AppTaskQueueDB {
    fn inmemory() -> Self {
        Self::InMemory(InMemoryQueue::new())
    }

    fn db(repository: AppPostgresRepository) -> Self {
        Self::DB(repository)
    }

    pub async fn enqueue_task(&self, task: AppTask) -> Result<()> {
        match self {
            AppTaskQueueDB::InMemory(inmemory) => inmemory.enqueue_task(task).await?,
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
            AppTaskQueueDB::InMemory(inmemory) => inmemory.peek_result(status_id).await,
            AppTaskQueueDB::DB(db) => db.peek_result(status_id).await,
        }
    }

    async fn execute_tasks<F, Fut>(&self, executor: F) -> Result<()>
    where
        F: FnOnce(MergedAppTask, HashSet<AppName>) -> Fut,
        Fut: Future<Output = std::result::Result<App, AppsError>>,
    {
        enum LockContext<'lc> {
            InMemory(<InMemoryQueue as AppTaskQueue>::LockContext<'lc>),
            DB(<AppPostgresRepository as AppTaskQueue>::LockContext<'lc>),
        }

        let (processable_tasks, lock) = match self {
            AppTaskQueueDB::InMemory(inmemory) => match inmemory.fetch_processable_tasks().await {
                Ok(Some((processable_tasks, lock))) => {
                    (processable_tasks, LockContext::InMemory(lock))
                }
                Ok(None) => {
                    return Ok(());
                }
                Err(err) => return Err(err.into()),
            },
            AppTaskQueueDB::DB(db) => match db.fetch_processable_tasks().await {
                Ok(Some((processable_tasks, lock))) => (processable_tasks, LockContext::DB(lock)),
                Ok(None) => {
                    return Ok(());
                }
                Err(err) => return Err(err.into()),
            },
        };

        let merged = AppTask::merge_tasks(processable_tasks.tasks);
        let result = match tokio::time::timeout(
            TASK_PROCESSING_TIMEOUT,
            executor(merged.clone(), processable_tasks.backed_up_tasks),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                log::error!("Processing of “{merged}” timed out");
                Err(AppsError::TaskProcessingTimedout {
                    duration: TASK_PROCESSING_TIMEOUT,
                    app_name: merged.app_name().clone(),
                })
            }
        };
        let done_timestamp = Utc::now();

        match (self, lock) {
            (AppTaskQueueDB::InMemory(inmemory), LockContext::InMemory(lock)) => {
                inmemory
                    .store_result(lock, result, done_timestamp, &merged)
                    .await?;
            }
            (AppTaskQueueDB::DB(db), LockContext::DB(lock)) => {
                db.store_result(lock, result, done_timestamp, &merged)
                    .await?;
            }
            _ => {
                unreachable!("This case must not ever happen")
            }
        }
        Ok(())
    }

    async fn clean_up_done_tasks(&self, older_than: DateTime<Utc>) -> Result<usize> {
        match self {
            AppTaskQueueDB::InMemory(inmemory) => {
                Ok(inmemory.clean_up_done_tasks(older_than).await?)
            }
            AppTaskQueueDB::DB(db) => Ok(db.clean_up_done_tasks(older_than).await?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DatabasePool;
    use domain::blueprint_service;
    use rstest::rstest;
    use std::collections::HashSet;
    use testcontainers_modules::{
        postgres::{self},
        testcontainers::{ContainerAsync, runners::AsyncRunner},
    };

    struct TestQueue {
        queue: AppTaskQueueDB,
        _test_container: Option<ContainerAsync<postgres::Postgres>>,
    }

    impl TestQueue {
        fn inmemory() -> Self {
            Self {
                queue: AppTaskQueueDB::inmemory(),
                _test_container: None,
            }
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

            Self {
                queue: AppTaskQueueDB::db(AppPostgresRepository::new(pool)),
                _test_container: Some(postgres_instance),
            }
        }
    }

    fn simulate_result(task: MergedAppTask) -> Result<App, AppsError> {
        Ok(App::new(
            vec![domain::app_instance::Service {
                id: task.task_to_work_on.status_id().to_string(),
                status: domain::app_instance::ServiceStatus::Paused,
                blueprint_config: domain::blueprint_service!("nginx"),
                service_type: domain::app_instance::ContainerType::Instance,
            }],
            HashSet::new(),
            None,
            None,
        ))
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
            .queue
            .enqueue_task(AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                status_id: AppStatusChangeId::new(),
                app_name: AppName::master(),
                infrastructure_payload_to_back_up: serde_json::json!({}),
            })
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Back-up & restore is not supported by in-memory app queue processing."
        );

        let err = queue
            .queue
            .enqueue_task(AppTask::RestoreOnInfrastructureAndDeleteFromBackup {
                status_id: AppStatusChangeId::new(),
                app_name: AppName::master(),
                infrastructure_payload_to_restore: serde_json::json!({}),
            })
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Back-up & restore is not supported by in-memory app queue processing."
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
            .queue
            .execute_tasks(async |_tasks, _| {
                unreachable!("Empty queue shouldn't trigger this code")
            })
            .await;

        assert!(result.is_ok())
    }

    #[rstest]
    #[case::inmemory(async { TestQueue::inmemory() }, Ok(App::new(
        vec![domain::app_instance::Service {
            id: String::from("nginx-1234"),
            status: domain::app_instance::ServiceStatus::Paused,
            blueprint_config: blueprint_service!("nginx"),
            service_type: domain::app_instance::ContainerType::ApplicationCompanion,
        }],
        HashSet::new(),
        None,
        None,
    )))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Ok(App::new(
        vec![domain::app_instance::Service {
            id: String::from("nginx-1234"),
            status: domain::app_instance::ServiceStatus::Paused,
            blueprint_config: blueprint_service!("nginx"),
            service_type: domain::app_instance::ContainerType::ApplicationCompanion,
        }],
        HashSet::new(),
        None,
        None,
    )))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Ok(App::new(
        vec![domain::app_instance::Service {
            id: String::from("nginx-1234"),
            status: domain::app_instance::ServiceStatus::Paused,
            blueprint_config: domain::blueprint_service!("nginx"),
            service_type: domain::app_instance::ContainerType::ApplicationCompanion,
        }],
        HashSet::new(),
        UserDefinedParameters::new(
            serde_json::json!("v0"),
            &jsonschema::Validator::new(&serde_json::json!({"type": "string"})).unwrap(),
        )
        .ok(),
        None,
    )))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::AppNotFound { app_name: AppName::master() }))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::AppLimitExceeded { limit: 10 }))]
    #[case::inmemory(async { TestQueue::inmemory() }, Err(AppsError::BaseRouteNotMergeable { error: String::from("error") }))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::BaseRouteNotMergeable { error: String::from("error") }))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::InfrastructureError { error: String::from("error") }))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::InvalidServerConfiguration { error: String::from("error") }))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::TemplatingIssue {
        error: crate::apps::DeploymentTemplatingError::TemplateRenderingError { err: String::from("error") }
    }))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::UnableToResolveImage {
        error: crate::registry::RegistryError::ImageNotFound { image: String::from("nginx") }
    }))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::UnapplicableHook {
        err: crate::apps::hooks::HooksError::Unexpected { err: String::from("error") }
    }))]
    #[case::postgres(async { TestQueue::postgres_queue().await }, Err(AppsError::InvalidUserDefinedParameters { err: String::from("error") }))]
    #[tokio::test]
    async fn enqueue_and_execute_single_task(
        #[future]
        #[case]
        queue: TestQueue,
        #[case] task_result: Result<App, AppsError>,
    ) {
        let queue = queue.await;

        let status_id = AppStatusChangeId::new();
        queue
            .queue
            .enqueue_task(AppTask::Delete {
                status_id,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let task_result_clone = task_result.clone();
        queue
            .queue
            .execute_tasks(async |_, _| task_result_clone)
            .await
            .unwrap();

        let result = queue.queue.peek_result(&status_id).await;
        assert_eq!(result, Some(task_result));

        let cleaned = queue.queue.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 1);
    }

    #[rstest]
    #[case::inmemory(async { TestQueue::inmemory() })]
    #[case::postgres(async { TestQueue::postgres_queue().await })]
    #[tokio::test]
    async fn handle_timeout(
        #[future]
        #[case]
        queue: TestQueue,
    ) {
        let queue = queue.await;

        let status_id = AppStatusChangeId::new();
        queue
            .queue
            .enqueue_task(AppTask::Delete {
                status_id,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        queue
            .queue
            .execute_tasks(async |tasks, _| {
                tokio::time::sleep(TASK_PROCESSING_TIMEOUT * 3).await;
                simulate_result(tasks)
            })
            .await
            .unwrap();

        let result = queue.queue.peek_result(&status_id).await;
        assert_eq!(
            result,
            Some(Err(AppsError::TaskProcessingTimedout {
                duration: TASK_PROCESSING_TIMEOUT,
                app_name: AppName::master(),
            }))
        );

        let cleaned = queue.queue.clean_up_done_tasks(Utc::now()).await.unwrap();
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
            .queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();
        let status_id_2 = AppStatusChangeId::new();
        queue
            .queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        queue
            .queue
            .execute_tasks(async |tasks, _| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.queue.peek_result(&status_id_2).await;
        assert!(matches!(result, Some(Ok(_))));

        let cleaned = queue.queue.clean_up_done_tasks(Utc::now()).await.unwrap();
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
            .queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let status_id_2 = AppStatusChangeId::new();
        queue
            .queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::from_str("other").unwrap(),
            })
            .await
            .unwrap();

        queue
            .queue
            .execute_tasks(async |tasks, _| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.queue.peek_result(&status_id_2).await;
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
            .queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let status_id_2 = AppStatusChangeId::new();
        queue
            .queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::from_str("other").unwrap(),
            })
            .await
            .unwrap();

        queue
            .queue
            .execute_tasks(async |tasks, _| simulate_result(tasks))
            .await
            .unwrap();
        queue
            .queue
            .execute_tasks(async |tasks, _| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.queue.peek_result(&status_id_2).await;
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
            .queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let status_id_2 = AppStatusChangeId::new();
        queue
            .queue
            .enqueue_task(AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                status_id: status_id_2,
                app_name: AppName::master(),
                infrastructure_payload_to_back_up: serde_json::json!({
                    "string-key": "test",
                    "array-key": [1, 2, 3]
                }),
            })
            .await
            .unwrap();

        queue
            .queue
            .execute_tasks(async |tasks, _| simulate_result(tasks))
            .await
            .unwrap();

        let result = queue.queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.queue.peek_result(&status_id_2).await;
        assert!(result.is_none());
    }

    mod processable_tasks {
        use super::super::*;
        use std::str::FromStr;

        #[test]
        fn fail_to_create_because_of_no_tasks_provided() {
            let error = ProcessableTasks::new(Vec::new(), Default::default()).unwrap_err();

            assert_eq!(error, ProcessableTasksError::MustProvideAtLeastOneTask)
        }

        #[test]
        fn fail_to_create_because_of_different_task_names() {
            let error = ProcessableTasks::new(
                vec![
                    AppTask::Delete {
                        status_id: AppStatusChangeId::new(),
                        app_name: AppName::master(),
                    },
                    AppTask::CreateOrUpdate {
                        status_id: AppStatusChangeId::new(),
                        app_name: AppName::from_str("other").unwrap(),
                        replicate_from: None,
                        service_configs: vec![],
                        owners: Default::default(),
                        user_defined_parameters: None,
                    },
                ],
                Default::default(),
            )
            .unwrap_err();

            assert_eq!(
                error,
                ProcessableTasksError::AppNamesDoNotMatch {
                    app_names: BTreeSet::from_iter([
                        AppName::master(),
                        AppName::from_str("other").unwrap()
                    ])
                }
            )
        }
    }
}
