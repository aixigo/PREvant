use std::{collections::HashSet, future::Future};

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
use sqlx::{postgres::PgConnectOptions, PgPool};

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

        let result = sqlx::query_as::<
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
        // TODO: error handling
        .unwrap();

        result.map(
            |(_id, app, error)| match (app.map(|app| app.0), error.map(|error| error.0)) {
                (Some(app), None) => Ok(app.into()),
                (None, Some(err)) => Err(err),
                _ => unreachable!(""),
            },
        )
    }

    pub async fn execute_task<F, Fut>(&self, f: F) -> Result<()>
    where
        F: FnOnce(AppTask) -> Fut,
        Fut: Future<Output = std::result::Result<App, AppsServiceError>>,
    {
        let mut tx = self.pool.begin().await?;

        let task = sqlx::query_as::<_, (sqlx::types::Uuid, sqlx::types::Json<AppTask>)>(
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
            let result = f(task.0).await;

            sqlx::query(
                r#"
                UPDATE app_task
                SET status = 'done', result_success = $3, result_error = $2
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(result.as_ref().map_or_else(
                |e| serde_json::to_value(e).ok(),
                |_| None,
            ))
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
