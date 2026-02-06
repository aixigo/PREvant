use crate::{
    apps::AppsError,
    models::{
        user_defined_parameters::UserDefinedParameters, App, AppName, AppStatusChangeId, AppTask,
        MergedAppTask, Owner, Service, ServiceConfig, ServiceStatus, State,
    },
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rocket::{
    fairing::{Fairing, Info, Kind},
    Build, Orbit, Rocket,
};
use sqlx::{PgPool, Postgres, Transaction};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    str::FromStr,
    sync::Mutex,
};
use tokio::sync::watch::Receiver;

pub struct AppRepository {
    backup_poller: Mutex<Option<BackupPoller>>,
}

impl AppRepository {
    pub fn fairing() -> Self {
        Self {
            backup_poller: Mutex::new(None),
        }
    }
}

#[rocket::async_trait]
impl Fairing for AppRepository {
    fn info(&self) -> Info {
        Info {
            name: "app-repository",
            kind: Kind::Ignite | Kind::Liftoff,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<Build>) -> rocket::fairing::Result {
        let repository = rocket
            .state::<PgPool>()
            .map(|pool| AppPostgresRepository::new(pool.clone()));

        match repository
            .as_ref()
            .map(|repository| repository.backup_updates())
        {
            Some((backup_poller, backup_updates)) => {
                log::debug!("Database is available, configuring backup poller to stream backup changes into application updates.");

                let mut bp = self.backup_poller.lock().unwrap();
                *bp = Some(backup_poller);

                Ok(rocket
                    .manage(repository)
                    .manage(Some(BackupUpdateReceiver(backup_updates))))
            }
            None => Ok(rocket
                .manage(None::<AppPostgresRepository>)
                .manage(None::<BackupUpdateReceiver>)),
        }
    }

    async fn on_liftoff(&self, _rocket: &Rocket<Orbit>) {
        let mut backup_poller = self.backup_poller.lock().unwrap();
        if let Some(backup_poller) = backup_poller.take() {
            tokio::task::spawn(backup_poller.0);
        }
    }
}

#[derive(Clone)]
pub struct AppPostgresRepository {
    pool: PgPool,
}

pub struct BackupUpdateReceiver(pub Receiver<HashMap<AppName, App>>);
struct BackupPoller(std::pin::Pin<Box<dyn Future<Output = ()> + Send>>);

impl AppPostgresRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn fetch_backup_infrastructure_payload(
        &self,
        app_name: &AppName,
    ) -> Result<Option<Vec<serde_json::Value>>> {
        let mut connection = self.pool.acquire().await?;

        let result = sqlx::query_as::<_, (sqlx::types::Json<serde_json::Value>,)>(
            r#"
                SELECT infrastructure_payload
                FROM app_backup
                WHERE app_name = $1
                "#,
        )
        .bind(app_name.as_str())
        .fetch_optional(&mut *connection)
        .await?;

        Ok(result.map(
            |mut infrastructure_payload| match infrastructure_payload.0.take() {
                serde_json::Value::Array(values) => values,
                v => vec![v],
            },
        ))
    }

    pub async fn fetch_backup_older_than(
        &self,
        older_than: DateTime<Utc>,
    ) -> Result<Vec<(AppName, DateTime<Utc>)>> {
        let mut connection = self.pool.acquire().await?;

        let result = sqlx::query_as::<_, (String, DateTime<Utc>)>(
            r#"
                SELECT app_name, created_at
                FROM app_backup
                WHERE created_at <= $1
                "#,
        )
        .bind(older_than)
        .fetch_all(&mut *connection)
        .await?;

        Ok(result
            .into_iter()
            .filter_map(|(app_name, created_at)| {
                Some((AppName::from_str(&app_name).ok()?, created_at))
            })
            .collect())
    }

    fn backup_updates(&self) -> (BackupPoller, Receiver<HashMap<AppName, App>>) {
        let (tx, rx) = tokio::sync::watch::channel::<HashMap<AppName, App>>(HashMap::new());

        let pool = self.pool.clone();
        let poller = BackupPoller(Box::pin(async move {
            loop {
                match pool.acquire().await {
                    Ok(mut connection) => {
                        log::debug!("Fetching list of backups to send updates.");
                        match Self::fetch_backed_up_apps_inner(&mut *connection).await {
                            Ok(apps) => {
                                tx.send_if_modified(move |state| {
                                    if &apps != state {
                                        log::debug!("List of backups changed, sending updates.");
                                        *state = apps;
                                        true
                                    } else {
                                        false
                                    }
                                });
                            }
                            Err(err) => {
                                log::error!("Cannot fetch backups from database: {err}");
                            }
                        }
                    }
                    Err(err) => {
                        log::error!("Fetching list of backups failed: {err}.");
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }));

        (poller, rx)
    }

    pub async fn fetch_backed_up_app(&self, app_name: &AppName) -> Result<Option<App>> {
        let mut connection = self.pool.acquire().await?;

        let result = sqlx::query_as::<_, (String, sqlx::types::Json<RawApp>)>(
            r#"
                SELECT app_name, app
                FROM app_backup
                WHERE app_name = $1
                "#,
        )
        .bind(app_name.as_str())
        .fetch_optional(&mut *connection)
        .await?;

        Ok(result.map(|(_app_name, app)| App::from(app.0)))
    }

    pub async fn fetch_backed_up_apps(&self) -> Result<HashMap<AppName, App>> {
        let mut connection = self.pool.acquire().await?;

        Self::fetch_backed_up_apps_inner(&mut *connection).await
    }

    async fn fetch_backed_up_apps_inner<'a, E>(executor: E) -> Result<HashMap<AppName, App>>
    where
        E: sqlx::Executor<'a, Database = Postgres>,
    {
        let result = sqlx::query_as::<_, (String, sqlx::types::Json<RawApp>)>(
            r#"
                SELECT app_name, app
                FROM app_backup
                "#,
        )
        .fetch_all(executor)
        .await?;

        Ok(result
            .into_iter()
            .map(|(app_name, app)| (AppName::from_str(&app_name).unwrap(), App::from(app.0)))
            .collect::<HashMap<_, _>>())
    }

    pub async fn enqueue_task(&self, task: AppTask) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO app_task (id, app_name, task)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(task.status_id().as_uuid())
        .bind(task.app_name().as_str())
        .bind(serde_json::to_value(&task).unwrap())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn peek_result(
        &self,
        status_id: &AppStatusChangeId,
    ) -> Option<std::result::Result<App, AppsError>> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .inspect_err(|err| log::error!("Cannot acquire database connection: {err}"))
            .ok()?;

        sqlx::query_as::<
            _,
            (
                Option<sqlx::types::Json<RawApp>>,
                Option<sqlx::types::Json<AppsError>>,
                Option<sqlx::types::Json<RawApp>>,
                Option<sqlx::types::Json<AppsError>>,
            ),
        >(
            r#"
            SELECT a.result_success, a.result_error, m.result_success, m.result_error
            FROM app_task a
                 LEFT OUTER JOIN app_task m
                 ON a.executed_and_merged_with = m.id
            WHERE a.id = $1
              AND a.status = 'done'
            "#,
        )
        .bind(status_id.as_uuid())
        .fetch_optional(&mut *connection)
        .await
        .inspect_err(|err| log::error!("Cannot peek result for {status_id}: {err}"))
        .ok()?
        .map(|(app, error, merged_app, merged_error)| {
            match (
                app.map(|app| app.0),
                error.map(|error| error.0),
                merged_app.map(|app| app.0),
                merged_error.map(|error| error.0),
            ) {
                (Some(app), None, None, None) => Ok(app.into()),
                (None, Some(err), None, None) => Err(err),
                (None, None, Some(app), None) => Ok(app.into()),
                (None, None, None, Some(err)) => Err(err),
                _ => unreachable!(
                    "There should be either a result or an error stored in the database"
                ),
            }
        })
    }

    /// All queued tasks will be locked by a new transaction (see [PostgreSQL as message
    /// queue](https://www.svix.com/resources/guides/postgres-message-queue/#why-use-postgresql-as-a-message-queue))
    /// and the `executor`'s result will be stored for the tasks that could be executed at once.
    pub async fn lock_queued_tasks_and_perform_executor<F, Fut>(&self, executor: F) -> Result<()>
    where
        F: FnOnce(Vec<AppTask>) -> Fut,
        Fut: Future<Output = (MergedAppTask, std::result::Result<App, AppsError>)>,
    {
        let mut tx = self.pool.begin().await?;

        let tasks = sqlx::query_as::<_, (sqlx::types::Uuid, sqlx::types::Json<AppTask>)>(
            r#"
            WITH eligible_tasks AS (
                SELECT id, task
                FROM app_task
                WHERE status = 'queued'
                AND app_name = (
                    SELECT app_name
                    FROM app_task
                    WHERE created_at = (SELECT min(created_at) FROM app_task WHERE status = 'queued')
                    AND status = 'queued'
                )
                ORDER BY created_at
                FOR UPDATE SKIP LOCKED
            )
            UPDATE app_task
            SET status = 'running'
            FROM eligible_tasks
            WHERE app_task.id = eligible_tasks.id
            RETURNING eligible_tasks .id, eligible_tasks.task;
            "#,
        )
        .fetch_all(&mut *tx)
        .await?;

        let tasks_to_work_on = tasks
            .iter()
            .map(|task_to_work_on| task_to_work_on.1 .0.clone())
            .collect::<Vec<_>>();

        if tasks_to_work_on.is_empty() {
            return Ok(());
        }

        let (merged_tasks, result) = executor(tasks_to_work_on).await;
        Self::store_result(
            &mut tx,
            merged_tasks.task_to_work_on,
            result,
            merged_tasks.tasks_to_be_marked_as_done,
        )
        .await?;

        Self::move_untouched_tasks_back_into_queue(&mut tx, merged_tasks.tasks_to_stay_untouched)
            .await?;

        tx.commit().await?;

        Ok(())
    }

    async fn store_result(
        tx: &mut Transaction<'_, Postgres>,
        tasked_worked_on: AppTask,
        result: Result<App, AppsError>,
        tasks_to_be_marked_as_done: HashSet<AppStatusChangeId>,
    ) -> Result<()> {
        let is_success = result.is_ok();
        let is_failed_deletion_due_to_app_not_found = result
            .as_ref()
            .map_err(|err| matches!(err, AppsError::AppNotFound { .. }))
            .map_or_else(|e| e, |_| false);
        let id = *tasked_worked_on.status_id();

        let failed_result = result
            .as_ref()
            .map_or_else(|e| serde_json::to_value(e).ok(), |_| None);
        let success_result = result.map_or_else(
            |_| None,
            |app| {
                let (services, owner, user_defined_parameters) =
                    app.into_services_and_owners_and_user_defined_parameters();
                let raw = RawApp {
                    owner,
                    services: services
                        .into_iter()
                        .map(|service| RawService {
                            id: service.id,
                            status: service.state.status,
                            config: service.config,
                        })
                        .collect(),
                    user_defined_parameters: user_defined_parameters.and_then(
                        |user_defined_parameters| {
                            serde_json::to_value(user_defined_parameters).ok()
                        },
                    ),
                };
                serde_json::to_value(raw).ok()
            },
        );

        sqlx::query(
            r#"
                UPDATE app_task
                SET status = 'done', result_success = $3, result_error = $2
                WHERE id = $1
                "#,
        )
        .bind(id.as_uuid())
        .bind(failed_result)
        .bind(&success_result)
        .execute(&mut **tx)
        .await?;

        match tasked_worked_on {
            AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                app_name,
                infrastructure_payload_to_back_up,
                ..
            } if is_success => {
                log::debug!("Backing-up infrastructure payload for {app_name}.");

                sqlx::query(
                    r#"
                    INSERT INTO app_backup (app_name, app, infrastructure_payload)
                    VALUES ($1, $2, $3);
                    "#,
                )
                .bind(app_name.as_str())
                .bind(success_result)
                .bind(serde_json::Value::Array(infrastructure_payload_to_back_up))
                .execute(&mut **tx)
                .await?;
            }
            AppTask::RestoreOnInfrastructureAndDeleteFromBackup { app_name, .. } if is_success => {
                log::debug!("Deleting infrastructure payload for {app_name} from backups.");

                sqlx::query(
                    r#"
                    DELETE FROM app_backup
                    WHERE app_name = $1;
                    "#,
                )
                .bind(app_name.as_str())
                .execute(&mut **tx)
                .await?;
            }
            AppTask::Delete { app_name, .. }
                if is_success || is_failed_deletion_due_to_app_not_found =>
            {
                log::debug!("Deleting infrastructure payload for {app_name} from backups due to deletion request.");

                sqlx::query(
                    r#"
                    DELETE FROM app_backup
                    WHERE app_name = $1;
                    "#,
                )
                .bind(app_name.as_str())
                .execute(&mut **tx)
                .await?;
            }
            _ => {}
        }

        for task_id_that_was_merged in tasks_to_be_marked_as_done {
            sqlx::query(
                r#"
                UPDATE app_task
                SET status = 'done', executed_and_merged_with = $1
                WHERE id = $2
                "#,
            )
            .bind(id.as_uuid())
            .bind(task_id_that_was_merged.as_uuid())
            .execute(&mut **tx)
            .await?;
        }

        Ok(())
    }

    async fn move_untouched_tasks_back_into_queue(
        tx: &mut Transaction<'_, Postgres>,
        tasks_to_stay_untouched: HashSet<AppStatusChangeId>,
    ) -> Result<()> {
        for task_id in tasks_to_stay_untouched {
            sqlx::query(
                r#"
                UPDATE app_task
                SET status = 'queued'
                WHERE id = $1
                "#,
            )
            .bind(task_id.as_uuid())
            .execute(&mut **tx)
            .await?;
        }

        Ok(())
    }

    pub async fn clean_up_done_tasks(&self, older_than: DateTime<Utc>) -> Result<usize> {
        let mut tx = self.pool.begin().await?;

        let affected_rows = sqlx::query(
            r#"
            DELETE FROM app_task
            WHERE status = 'done'
              AND created_at <= $1
            "#,
        )
        .bind(older_than)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        tx.commit().await?;

        Ok(affected_rows as usize)
    }
}

#[derive(Serialize, Deserialize)]
struct RawApp {
    services: Vec<RawService>,
    owner: HashSet<Owner>,
    user_defined_parameters: Option<serde_json::Value>,
}

impl From<RawApp> for App {
    fn from(value: RawApp) -> Self {
        Self::new(
            value.services.into_iter().map(Service::from).collect(),
            Owner::normalize(value.owner),
            value
                .user_defined_parameters
                .map(|data| unsafe { UserDefinedParameters::without_validation(data) }),
            None,
        )
    }
}

#[derive(Serialize, Deserialize)]
struct RawService {
    id: String,
    status: ServiceStatus,
    config: ServiceConfig,
}

impl From<RawService> for Service {
    fn from(value: RawService) -> Self {
        Self {
            id: value.id,
            state: State {
                status: value.status,
                started_at: None,
            },
            config: value.config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::DatabasePool, models::AppName, sc};
    use sqlx::postgres::PgConnectOptions;
    use std::{str::FromStr, time::Duration};
    use testcontainers_modules::{
        postgres::{self},
        testcontainers::{runners::AsyncRunner, ContainerAsync},
    };

    async fn create_repository() -> (ContainerAsync<postgres::Postgres>, AppPostgresRepository) {
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

        (postgres_instance, AppPostgresRepository { pool })
    }

    #[tokio::test]
    async fn enqueue_and_execute_successfully() {
        let (_postgres_instance, repository) = create_repository().await;

        let status_id = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::Delete {
                status_id,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        repository
            .lock_queued_tasks_and_perform_executor(async |tasks| {
                let merged = AppTask::merge_tasks(tasks);
                (
                    merged,
                    Ok(App::new(
                        vec![Service {
                            id: String::from("nginx-1234"),
                            state: State {
                                status: ServiceStatus::Paused,
                                started_at: None,
                            },
                            config: sc!("nginx"),
                        }],
                        HashSet::new(),
                        None,
                        None,
                    )),
                )
            })
            .await
            .unwrap();

        let result = repository.peek_result(&status_id).await;
        assert!(matches!(result, Some(Ok(_))));

        let cleaned = repository.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 1);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(Ok(App::new(
                vec![Service {
                    id: String::from("nginx-1234"),
                    state: State {
                        status: ServiceStatus::Paused,
                        started_at: None,
                    },
                    config: sc!("nginx"),
                }],
                HashSet::new(),
                None,
                None,
    )))]
    // simulate that app has been deleted via kubectl or another while the update was in the
    // database
    #[case(Err(AppsError::AppNotFound { app_name: AppName::master() }))]
    async fn clean_up_back_up_after_deletion(#[case] delete_task_result: Result<App, AppsError>) {
        let (_postgres_instance, repository) = create_repository().await;

        let status_id = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                status_id,
                app_name: AppName::master(),
                infrastructure_payload_to_back_up: vec![serde_json::json!({})],
            })
            .await
            .unwrap();

        repository
            .lock_queued_tasks_and_perform_executor(async |tasks| {
                assert_eq!(tasks.len(), 1);

                let merged = AppTask::merge_tasks(tasks);
                assert!(matches!(
                    merged.task_to_work_on,
                    AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure { .. }
                ));

                (
                    merged,
                    Ok(App::new(
                        vec![Service {
                            id: String::from("nginx-1234"),
                            state: State {
                                status: ServiceStatus::Paused,
                                started_at: None,
                            },
                            config: sc!("nginx"),
                        }],
                        HashSet::new(),
                        None,
                        None,
                    )),
                )
            })
            .await
            .unwrap();

        let status_id = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::Delete {
                status_id,
                app_name: AppName::master(),
            })
            .await
            .unwrap();
        repository
            .lock_queued_tasks_and_perform_executor(async |tasks| {
                assert_eq!(tasks.len(), 1);
                assert!(matches!(tasks[0], AppTask::Delete { .. }));

                let merged = AppTask::merge_tasks(tasks);

                (merged, delete_task_result)
            })
            .await
            .unwrap();

        let backups = repository.fetch_backed_up_apps().await.unwrap();
        assert!(backups.is_empty());
    }

    #[tokio::test]
    async fn do_not_clean_up_back_up_after_failed_deletion() {
        let (_postgres_instance, repository) = create_repository().await;

        let status_id = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
                status_id,
                app_name: AppName::master(),
                infrastructure_payload_to_back_up: vec![serde_json::json!({})],
            })
            .await
            .unwrap();

        repository
            .lock_queued_tasks_and_perform_executor(async |tasks| {
                assert_eq!(tasks.len(), 1);

                let merged = AppTask::merge_tasks(tasks);
                assert!(matches!(
                    merged.task_to_work_on,
                    AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure { .. }
                ));

                (
                    merged,
                    Ok(App::new(
                        vec![Service {
                            id: String::from("nginx-1234"),
                            state: State {
                                status: ServiceStatus::Paused,
                                started_at: None,
                            },
                            config: sc!("nginx"),
                        }],
                        HashSet::new(),
                        None,
                        None,
                    )),
                )
            })
            .await
            .unwrap();

        let status_id = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::Delete {
                status_id,
                app_name: AppName::master(),
            })
            .await
            .unwrap();
        repository
            .lock_queued_tasks_and_perform_executor(async |tasks| {
                assert_eq!(tasks.len(), 1);
                assert!(matches!(tasks[0], AppTask::Delete { .. }));

                let merged = AppTask::merge_tasks(tasks);

                (
                    merged,
                    Err(AppsError::InfrastructureError {
                        error: String::from("unexpected"),
                    }),
                )
            })
            .await
            .unwrap();

        let backup = repository
            .fetch_backed_up_app(&AppName::master())
            .await
            .unwrap();
        assert_eq!(
            backup,
            Some(App::new(
                vec![Service {
                    id: String::from("nginx-1234"),
                    state: State {
                        status: ServiceStatus::Paused,
                        started_at: None,
                    },
                    config: sc!("nginx").with_port(0),
                }],
                HashSet::new(),
                None,
                None,
            ))
        );
    }

    #[tokio::test]
    async fn execute_all_tasks_per_app_name() {
        let (_postgres_instance, repository) = create_repository().await;

        let status_id_1 = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();
        let status_id_2 = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        // spawning an independent task that asserts that all tasks for AppName::master() are
        // blocked by the execute_task below.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let spawned_repository = AppPostgresRepository {
            pool: repository.pool.clone(),
        };
        let spawn_handle_1 = tokio::spawn(async move {
            rx.await.unwrap();
            spawned_repository
                .lock_queued_tasks_and_perform_executor(async |tasks| {
                    unreachable!("There should be no task to be executed here because the spawned task should have blocked it: {tasks:?}")
                })
                .await
        });

        let spawned_repository = AppPostgresRepository {
            pool: repository.pool.clone(),
        };

        let spawn_handle_2 = tokio::spawn(async move {
            spawned_repository
                .lock_queued_tasks_and_perform_executor(async |tasks| {
                    tx.send(()).unwrap();

                    tokio::time::sleep(Duration::from_secs(4)).await;

                    let merged = AppTask::merge_tasks(tasks);
                    (
                        merged,
                        Ok(App::new(
                            vec![Service {
                                id: String::from("nginx-1234"),
                                state: State {
                                    status: ServiceStatus::Paused,
                                    started_at: None,
                                },
                                config: sc!("nginx"),
                            }],
                            HashSet::new(),
                            None,
                            None,
                        )),
                    )
                })
                .await
        });

        let result_1 = spawn_handle_1.await;
        assert!(result_1.is_ok());
        let result_2 = spawn_handle_2.await;
        assert!(result_2.is_ok());

        let result_1 = repository.peek_result(&status_id_1).await;
        assert!(matches!(result_1, Some(Ok(_))));
        let result_2 = repository.peek_result(&status_id_2).await;
        assert_eq!(result_2, result_1);

        let cleaned = repository.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 2);
    }

    #[tokio::test]
    async fn execute_task_with_different_app_names_in_parallel() {
        let (_postgres_instance, repository) = create_repository().await;

        let status_id_1 = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1,
                app_name: AppName::master(),
            })
            .await
            .unwrap();
        let status_id_2 = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::from_str("other").unwrap(),
            })
            .await
            .unwrap();
        let status_id_3 = AppStatusChangeId::new();
        repository
            .enqueue_task(AppTask::Delete {
                status_id: status_id_3,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let spawned_repository = AppPostgresRepository {
            pool: repository.pool.clone(),
        };
        let spawn_handle_1 = tokio::spawn(async move {
            spawned_repository
                .lock_queued_tasks_and_perform_executor(async |tasks| {
                    let merged = AppTask::merge_tasks(tasks);
                    (
                        merged,
                        Ok(App::new(
                            vec![Service {
                                id: String::from("nginx-1234"),
                                state: State {
                                    status: ServiceStatus::Paused,
                                    started_at: None,
                                },
                                config: sc!("nginx"),
                            }],
                            HashSet::new(),
                            None,
                            None,
                        )),
                    )
                })
                .await
                .unwrap();
        });

        let spawned_repository = AppPostgresRepository {
            pool: repository.pool.clone(),
        };
        let spawn_handle_2 = tokio::spawn(async move {
            spawned_repository
                .lock_queued_tasks_and_perform_executor(async |tasks| {
                    let merged = AppTask::merge_tasks(tasks);
                    (
                        merged,
                        Ok(App::new(
                            vec![Service {
                                id: String::from("nginx-1234"),
                                state: State {
                                    status: ServiceStatus::Paused,
                                    started_at: None,
                                },
                                config: sc!("nginx"),
                            }],
                            HashSet::new(),
                            None,
                            None,
                        )),
                    )
                })
                .await
                .unwrap();
        });

        let result_1 = spawn_handle_1.await;
        assert!(result_1.is_ok());
        let result_2 = spawn_handle_2.await;
        assert!(result_2.is_ok());

        let result_from_master = repository.peek_result(&status_id_1).await;
        assert!(matches!(result_from_master, Some(Ok(_))));
        let result = repository.peek_result(&status_id_2).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = repository.peek_result(&status_id_3).await;
        assert_eq!(result, result_from_master);
    }
}
