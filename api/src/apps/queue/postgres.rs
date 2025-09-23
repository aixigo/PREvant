use std::{collections::HashSet, future::Future};

use super::AppTask;
use crate::{
    apps::AppsServiceError,
    models::{
        user_defined_parameters::{self, UserDefinedParameters},
        App, AppStatusChangeId, Owner, Service, ServiceConfig, ServiceStatus, State,
    },
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{
    postgres::{PgConnectOptions, PgRow},
    FromRow, PgPool, Row,
};

pub struct PostgresAppTaskQueueDB {
    pool: PgPool,
}

struct AppTaskResult {
    ok: Option<App>,
    error: Option<AppsServiceError>,
}

#[derive(Serialize, Deserialize)]
struct RawApp {
    services: Vec<RawService>,
    owner: HashSet<Owner>,
    // user_defined_parameters: Option<UserDefinedParameters>,
}

#[derive(Serialize, Deserialize)]
struct RawService {
    id: String,
    status: ServiceStatus,
    config: ServiceConfig,
}

impl<'r> FromRow<'r, PgRow> for AppTaskResult {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        let result_success: Option<Value> = row.try_get("result_success")?;
        let result_error: Option<Value> = row.try_get("result_error")?;

        Ok(Self {
            ok: result_success.and_then(|app| {
                let raw = serde_json::from_value::<RawApp>(app).ok()?;

                Some(App::new(
                    raw.services
                        .into_iter()
                        .map(|raw_service| Service {
                            id: raw_service.id,
                            state: State {
                                status: raw_service.status,
                                started_at: None,
                            },
                            config: raw_service.config,
                        })
                        .collect(),
                    raw.owner,
                    // TODO user_defined_parameters
                    None,
                ))
            }),
            error: result_error
                .and_then(|err| serde_json::from_value::<AppsServiceError>(err).ok()),
        })
    }
}

impl PostgresAppTaskQueueDB {
    pub async fn connect(database_options: PgConnectOptions) -> Result<Self> {
        let pool = PgPool::connect_with(database_options).await?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!().run(&self.pool).await?;
        Ok(())
    }

    pub async fn enqueue_task(&self, task: AppTask) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO app_task (id, task)
            VALUES ($1, $2)
            "#,
        )
        .bind(task.status_id().as_uuid())
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
            // TODO: error handling
            .unwrap();

        let result = sqlx::query_as::<_, AppTaskResult>(
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
        // TODO: error handling
        .unwrap();

        result.map(|result| match (result.ok, result.error) {
            (Some(ok), None) => Ok(ok),
            (None, Some(err)) => Err(err),
            _ => unreachable!(""),
        })
    }

    pub async fn execute_task<F, Fut>(&self, f: F) -> Result<()>
    where
        F: FnOnce(AppTask) -> Fut,
        Fut: Future<Output = std::result::Result<App, AppsServiceError>>,
    {
        let mut tx = self.pool.begin().await?;

        let task = sqlx::query_as::<_, (sqlx::types::Uuid, sqlx::types::JsonValue)>(
            r#"
            WITH cte AS (
                SELECT id, task
                FROM app_task
                WHERE status = 'new'
                ORDER BY created_at
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            UPDATE app_task
            SET status = 'inProcess'
            FROM cte
            WHERE app_task.id = cte.id
            RETURNING cte.id, cte.task;
            "#,
        )
        .fetch_optional(&mut *tx)
        .await?;

        if let Some((id, task)) = task {
            let task = serde_json::from_value::<AppTask>(task)?;

            let result = f(task).await;
            sqlx::query(
                r#"
                UPDATE app_task
                SET status = 'done', result_success = $3, result_error = $2
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(result.as_ref().map_or_else(
                |e| serde_json::to_value(e).unwrap(),
                |_| serde_json::Value::Null,
            ))
            .bind(result.map_or_else(
                |_| serde_json::Value::Null,
                |app| {
                    // TODO: user_defined_parameters
                    let (services, owner, _user_defined_parameters) =
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
                    };
                    serde_json::to_value(raw).unwrap()
                },
            ))
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
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
