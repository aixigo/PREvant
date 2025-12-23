use crate::models::AppName;
use anyhow::Result;
use rocket::{
    fairing::{Fairing, Info, Kind},
    Build, Rocket,
};
use sqlx::PgPool;

pub struct AppRepository {}

impl AppRepository {
    pub fn fairing() -> Self {
        Self {}
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
            .map(|pool| AppPostgresRepository { pool: pool.clone() });
        Ok(rocket.manage(repository))
    }
}

pub struct AppPostgresRepository {
    pool: PgPool,
}

impl AppPostgresRepository {
    pub async fn fetch_backup(&self, app_name: &AppName) -> Result<Option<Vec<serde_json::Value>>> {
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
}
