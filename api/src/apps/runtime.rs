use crate::{
    apps::repository::AppPostgresRepository,
    infrastructure::{Infrastructure, InfrastructureProxy},
};
use domain::{
    AppName,
    app_instance::{App, BackedUpApp, RuntimeInfrastructure},
    app_life_cycle::BackupRepository,
};
use either::Either;
use std::collections::HashMap;

pub struct RuntimeInfrastructureAdapter {
    infrastructure: InfrastructureProxy,
    repository: Option<AppPostgresRepository>,
}

impl RuntimeInfrastructureAdapter {
    pub(super) fn new(
        infrastructure: InfrastructureProxy,
        repository: Option<AppPostgresRepository>,
    ) -> Self {
        Self {
            infrastructure,
            repository,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum RuntimeInfrastructureAdapterError {
    #[error("{0}")]
    Infrastructure(
        // TODO: should be replaced
        anyhow::Error,
    ),
    #[error("{0}")]
    BackupRepository(<AppPostgresRepository as BackupRepository>::Error),
}

#[async_trait::async_trait]
impl RuntimeInfrastructure for RuntimeInfrastructureAdapter {
    type BackupPayload = <InfrastructureProxy as Infrastructure>::BackupPayload;
    type Error = RuntimeInfrastructureAdapterError;

    async fn fetch_apps(
        &self,
    ) -> Result<HashMap<AppName, Either<App, BackedUpApp<Self::BackupPayload>>>, Self::Error> {
        let (apps, app_backups) = futures::try_join!(
            async {
                self.infrastructure
                    .fetch_apps()
                    .await
                    .map_err(|e| todo!("{e}"))
            },
            async {
                match &self.repository {
                    Some(repository) => repository
                        .fetch_backed_up_apps()
                        .await
                        .map_err(RuntimeInfrastructureAdapterError::BackupRepository),
                    None => Ok(HashMap::new()),
                }
            }
        )?;

        let mut all_apps = HashMap::with_capacity(apps.len() + app_backups.len());

        for (app_name, backed_up_app) in app_backups.into_iter() {
            all_apps.insert(
                app_name,
                Either::Right(BackedUpApp::with_app_and_payload(
                    backed_up_app,
                    todo!("payload from DB should be turned into backup payload"),
                )),
            );
        }

        for (app_name, app) in apps.into_iter() {
            all_apps.entry(app_name).or_insert(Either::Left(app));
        }

        Ok(all_apps)
    }

    async fn fetch_app(
        &self,
        app_name: AppName,
    ) -> Result<Option<Either<App, BackedUpApp<Self::BackupPayload>>>, Self::Error> {
        let (app, backed_up_app) = futures::try_join!(
            async {
                self.infrastructure
                    .fetch_app(&app_name)
                    .await
                    .map_err(RuntimeInfrastructureAdapterError::Infrastructure)
            },
            async {
                match &self.repository {
                    Some(repository) => repository
                        .load_backed_up_app(&app_name)
                        .await
                        .map_err(|e| RuntimeInfrastructureAdapterError::BackupRepository(dbg!(e))),
                    None => Ok(None),
                }
            }
        )?;

        match (app, backed_up_app) {
            (None, None) => Ok(None),
            (_, Some(backed_up_app)) => Ok(Some(Either::Right(backed_up_app))),
            (Some(app), None) => Ok(Some(Either::Left(app))),
        }
    }
}
