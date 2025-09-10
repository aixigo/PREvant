use super::AppTask;
use crate::{
    apps::AppsServiceError,
    models::{
        user_defined_parameters::UserDefinedParameters, App, AppStatusChangeId, Owner, Service,
        ServiceConfig, ServiceStatus, State,
    },
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use exponential_backoff::Backoff;
use sqlx::{postgres::PgConnectOptions, PgPool};
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
    pub async fn connect_with_exponential_backoff(
        database_options: PgConnectOptions,
    ) -> Result<Self> {
        let min = std::time::Duration::from_millis(100);
        let max = std::time::Duration::from_secs(10);
        for duration in Backoff::new(5, min, max) {
            let pool = match PgPool::connect_with(database_options.clone()).await {
                Ok(pool) => pool,
                Err(err) => match duration {
                    Some(duration) => {
                        log::warn!("Cannot connect to database, trying again: {err}");
                        tokio::time::sleep(duration).await;
                        continue;
                    }
                    None => {
                        return Err(err)?;
                    }
                },
            };
            return Ok(Self { pool });
        }
        unreachable!()
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!().run(&self.pool).await?;
        Ok(())
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
    ) -> Option<std::result::Result<App, AppsServiceError>> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .inspect_err(|err| log::error!("Cannot acquire database connection: {err}"))
            .ok()?;

        sqlx::query_as::<
            _,
            (
                sqlx::types::Uuid,
                Option<sqlx::types::Json<RawApp>>,
                Option<sqlx::types::Json<AppsServiceError>>,
            ),
        >(
            r#"
            SELECT id, result_success, result_error
            FROM app_task
            WHERE id = $1
              AND status = 'done'
            "#,
        )
        .bind(status_id.as_uuid())
        .fetch_optional(&mut *connection)
        .await
        .inspect_err(|err| log::error!("Cannot peek result: {err}"))
        .ok()?
        .map(|(_id, app, error)| {
            match (app.map(|app| app.0), error.map(|error| error.0)) {
                (Some(app), None) => Ok(app.into()),
                (None, Some(err)) => Err(err),
                _ => unreachable!(
                    "There should be either a result or an error stored in the database"
                ),
            }
        })
    }

    pub async fn execute_task<F, Fut>(&self, f: F) -> Result<()>
    where
        F: FnOnce(AppTask) -> Fut,
        Fut: Future<Output = std::result::Result<App, AppsServiceError>>,
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

        let mut tasks = tasks.into_iter();

        let Some((id, task_to_work_one)) = tasks.next() else {
            return Ok(());
        };

        let result = f(task_to_work_one.0).await;

        sqlx::query(
            r#"
                UPDATE app_task
                SET status = 'done', result_success = $3, result_error = $2
                WHERE id = $1
                "#,
        )
        .bind(id)
        .bind(
            result
                .as_ref()
                .map_or_else(|e| serde_json::to_value(e).ok(), |_| None),
        )
        .bind(result.map_or_else(
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
        ))
        .execute(&mut *tx)
        .await?;

        for (id, _task_that_was_blocked) in tasks {
            sqlx::query(
                r#"
                UPDATE app_task
                SET status = 'queued'
                WHERE id = $1
                "#,
            )
            .bind(id)
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
    use crate::{models::AppName, sc};
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

        let queue = PostgresAppTaskQueueDB::connect_with_exponential_backoff(connection)
            .await
            .unwrap();
        queue.migrate().await.unwrap();

        (postgres_instance, queue)
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
            .execute_task(async |_task| {
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
                ))
            })
            .await
            .unwrap();

        let result = queue.peek_result(&status_id).await;
        assert!(matches!(result, Some(Ok(_))));

        let cleaned = queue.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 1);
    }

    #[tokio::test]
    async fn execute_one_task_per_app_name() {
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
                .execute_task(async |task| {
                    unreachable!("There should be no task to be executed here because the spawned task should have blocked it: {task:?}")
                })
                .await
        });

        let spawned_queue = PostgresAppTaskQueueDB {
            pool: queue.pool.clone(),
        };

        let spawn_handle_2 = tokio::spawn(async move {
            spawned_queue
                .execute_task(async |_| {
                    tx.send(()).unwrap();

                    tokio::time::sleep(Duration::from_secs(4)).await;

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
                    ))
                })
                .await
        });

        let result_1 = spawn_handle_1.await;
        assert!(result_1.is_ok());
        let result_2 = spawn_handle_2.await;
        assert!(result_2.is_ok());

        let result_1 = queue.peek_result(&status_id_1).await;
        assert!(matches!(result_1, Some(Ok(_))));

        let cleaned = queue.clean_up_done_tasks(Utc::now()).await.unwrap();
        assert_eq!(cleaned, 1);

        let result_2 = queue.peek_result(&status_id_2).await;
        assert!(matches!(result_2, None));
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
        queue
            .enqueue_task(AppTask::Delete {
                status_id: AppStatusChangeId::new(),
                app_name: AppName::master(),
            })
            .await
            .unwrap();

        let spawned_queue = PostgresAppTaskQueueDB {
            pool: queue.pool.clone(),
        };
        let spawn_handle_1 = tokio::spawn(async move {
            spawned_queue
                .execute_task(async |_task| {
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
                    ))
                })
                .await
                .unwrap();
        });

        let spawned_queue = PostgresAppTaskQueueDB {
            pool: queue.pool.clone(),
        };
        let spawn_handle_2 = tokio::spawn(async move {
            spawned_queue
                .execute_task(async |_task| {
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
                    ))
                })
                .await
                .unwrap();
        });

        let result_1 = spawn_handle_1.await;
        assert!(result_1.is_ok());
        let result_2 = spawn_handle_2.await;
        assert!(result_2.is_ok());

        let result = queue.peek_result(&status_id_1).await;
        assert!(matches!(result, Some(Ok(_))));
        let result = queue.peek_result(&status_id_2).await;
        assert!(matches!(result, Some(Ok(_))));
    }
}
