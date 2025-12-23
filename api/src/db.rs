use crate::config::Config;
use anyhow::Result;
use rocket::{
    fairing::{Fairing, Info, Kind},
    Build, Orbit, Rocket,
};
use sqlx::{postgres::PgConnectOptions, PgPool};

pub struct DatabasePool {}

impl DatabasePool {
    pub fn fairing() -> Self {
        Self {}
    }

    pub async fn connect_with_exponential_backoff(
        database_options: PgConnectOptions,
    ) -> Result<PgPool> {
        let min = std::time::Duration::from_millis(100);
        let max = std::time::Duration::from_secs(10);
        for duration in exponential_backoff::Backoff::new(5, min, max) {
            log::debug!("Connecting to database…");
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
            return Ok(pool);
        }
        unreachable!()
    }
}

#[rocket::async_trait]
impl Fairing for DatabasePool {
    fn info(&self) -> Info {
        Info {
            name: "database-pool",
            kind: Kind::Ignite | Kind::Liftoff,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<Build>) -> rocket::fairing::Result {
        match rocket
            .state::<Config>()
            .and_then(|config| config.database.as_ref())
        {
            Some(db_opitons) => {
                match Self::connect_with_exponential_backoff(db_opitons.clone()).await {
                    Ok(pool) => Ok(rocket.manage(pool)),
                    Err(err) => {
                        log::error!("Cannot connet to database: {err}");
                        Err(rocket)
                    }
                }
            }
            None => Ok(rocket),
        }
    }

    async fn on_liftoff(&self, rocket: &Rocket<Orbit>) {
        if let Some(pool) = rocket.state::<PgPool>() {
            if let Err(err) = sqlx::migrate!().run(pool).await {
                log::error!("Cannot apply database migration: {err}");
                rocket.shutdown().notify();
            }
        }
    }
}
