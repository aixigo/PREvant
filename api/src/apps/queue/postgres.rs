use super::AppTask;
use crate::{
    apps::AppsError,
    models::{
        user_defined_parameters::UserDefinedParameters, App, AppStatusChangeId, Owner, Service,
        ServiceConfig, ServiceStatus, State,
    },
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::{collections::HashSet, future::Future};

pub struct PostgresAppTaskQueueDB {
    pool: PgPool,
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

impl PostgresAppTaskQueueDB {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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

    pub async fn execute_tasks<F, Fut>(&self, f: F) -> Result<()>
    where
        F: FnOnce(Vec<AppTask>) -> Fut,
        Fut: Future<Output = (AppTask, std::result::Result<App, AppsError>)>,
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

        let (tasked_worked_on, result) = f(tasks_to_work_on).await;
        let is_success = result.is_ok();
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
        .execute(&mut *tx)
        .await?;

        if let AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure {
            app_name,
            infrastructure_payload,
            ..
        } = tasked_worked_on
        {
            if is_success {
                sqlx::query(
                    r#"
                    INSERT INTO app_backup (app_name, app, infrastructure_payload)
                    VALUES ($1, $2, $3);
                    "#,
                )
                .bind(app_name.as_str())
                .bind(success_result)
                .bind(infrastructure_payload)
                .execute(&mut *tx)
                .await?;
            }
        }

        // TODO: backup cannot be merged… and we should mark up to this task id.
        for (task_id_that_was_merged, _merged_task) in
            tasks.iter().filter(|task| task.0 != *id.as_uuid())
        {
            sqlx::query(
                r#"
                UPDATE app_task
                SET status = 'done', executed_and_merged_with = $1
                WHERE id = $2
                "#,
            )
            .bind(id.as_uuid())
            .bind(task_id_that_was_merged)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

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

#[cfg(test)]
mod tests {
    use std::{str::FromStr, time::Duration};

    use super::*;
    use crate::{db::DatabasePool, models::AppName, sc};
    use sqlx::postgres::PgConnectOptions;
    use testcontainers_modules::{
        postgres::{self},
        testcontainers::{runners::AsyncRunner, ContainerAsync},
    };

    async fn create_queue() -> (ContainerAsync<postgres::Postgres>, PostgresAppTaskQueueDB) {
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

        (postgres_instance, PostgresAppTaskQueueDB::new(pool))
    }

    #[tokio::test]
    async fn enqueue_and_execute_successfully() {
        let (_postgres_instance, queue) = create_queue().await;

        let status_id = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id.clone(),
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        queue
            .execute_tasks(async |tasks| {
                let task = tasks.last().unwrap().clone();
                (
                    task,
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
                    )),
                )
            })
            .await
            .unwrap();

        let result = queue.peek_result(&status_id).await;
        assert!(matches!(result, Some(Ok(_))));

        let cleaned = queue.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 1);
    }

    #[tokio::test]
    async fn execute_all_tasks_per_app_name() {
        let (_postgres_instance, queue) = create_queue().await;

        let status_id_1 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1.clone(),
                app_name: AppName::master(),
            })
            .await
            .unwrap();
        let status_id_2 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2.clone(),
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        // spawning an independent task that asserts that all tasks for AppName::master() are
        // blocked by the execute_task below.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let spawned_queue = PostgresAppTaskQueueDB {
            pool: queue.pool.clone(),
        };
        let spawn_handle_1 = tokio::spawn(async move {
            let _ = rx.await.unwrap();
            spawned_queue
                .execute_tasks(async |tasks| {
                    unreachable!("There should be no task to be executed here because the spawned task should have blocked it: {tasks:?}")
                })
                .await
        });

        let spawned_queue = PostgresAppTaskQueueDB {
            pool: queue.pool.clone(),
        };

        let spawn_handle_2 = tokio::spawn(async move {
            spawned_queue
                .execute_tasks(async |tasks| {
                    tx.send(()).unwrap();

                    tokio::time::sleep(Duration::from_secs(4)).await;

                    let task = tasks.last().unwrap().clone();
                    (
                        task,
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
                        )),
                    )
                })
                .await
        });

        let result_1 = spawn_handle_1.await;
        assert!(result_1.is_ok());
        let result_2 = spawn_handle_2.await;
        assert!(result_2.is_ok());

        let result_1 = queue.peek_result(&status_id_1).await;
        assert!(matches!(result_1, Some(Ok(_))));
        let result_2 = queue.peek_result(&status_id_2).await;
        assert_eq!(result_2, result_1);

        let cleaned = queue.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 2);
    }

    #[tokio::test]
    async fn execute_task_with_different_app_names_in_parallel() {
        let (_postgres_instance, queue) = create_queue().await;

        let status_id_1 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_1.clone(),
                app_name: AppName::master(),
            })
            .await
            .unwrap();
        let status_id_2 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_2.clone(),
                app_name: AppName::from_str("other").unwrap(),
            })
            .await
            .unwrap();
        let status_id_3 = AppStatusChangeId::new();
        queue
            .enqueue_task(AppTask::Delete {
                status_id: status_id_3,
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let spawned_queue = PostgresAppTaskQueueDB {
            pool: queue.pool.clone(),
        };
        let spawn_handle_1 = tokio::spawn(async move {
            spawned_queue
                .execute_tasks(async |tasks| {
                    let task = tasks.last().unwrap().clone();
                    (
                        task,
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
                        )),
                    )
                })
                .await
                .unwrap();
        });

        let spawned_queue = PostgresAppTaskQueueDB {
            pool: queue.pool.clone(),
        };
        let spawn_handle_2 = tokio::spawn(async move {
            spawned_queue
                .execute_tasks(async |tasks| {
                    let task = tasks.last().unwrap().clone();
                    (
                        task,
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

        let result_from_master = queue.peek_result(&status_id_1).await;
        assert!(matches!(result_from_master, Some(Ok(_))));
        let result = queue.peek_result(&status_id_2).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.peek_result(&status_id_3).await;
        assert_eq!(result, result_from_master);
    }
}
