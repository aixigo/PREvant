//! TODO
//!
use crate::{
    AppName,
    app_instance::{App, BackedUpApp, BackedUpAppError},
};

#[async_trait::async_trait]
pub trait LifecycleInfrastructure {
    type BackedUpElement;
    type Error;

    async fn snapshot_for_backup(
        &self,
        app_name: &AppName,
    ) -> Result<Option<Vec<Self::BackedUpElement>>, Self::Error>;

    async fn suspend_backed_up_elements(
        &mut self,
        app_name: &AppName,
        backed_up_elements: &[Self::BackedUpElement],
    ) -> Result<(), Self::Error>;

    async fn restore_from_backup(
        &mut self,
        app_name: &AppName,
        backed_up_elements: Vec<Self::BackedUpElement>,
    ) -> Result<Vec<Self::BackedUpElement>, Self::Error>;
}

#[async_trait::async_trait]
pub trait BackupRepository {
    type BackedUpElement;
    type Error;

    async fn store_elements_for_backup(
        &mut self,
        app_name: &AppName,
        backed_up_elements: Vec<Self::BackedUpElement>,
    ) -> Result<Vec<Self::BackedUpElement>, Self::Error>;

    async fn delete_restored_elements(
        &mut self,
        app_name: &AppName,
        restored_elements: Vec<Self::BackedUpElement>,
    ) -> Result<(), Self::Error>;
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum AppLifeCycleError<LifecycleInfrastructureError, BackupRepositoryError> {
    #[error("There are no infrastructure elements available")]
    NoBackUpSnapshotAvailable,
    #[error("There is no backup available")]
    NoBackUpAvailable,
    #[error("Failed to interact with the infrastructure during back-up & restore: {0}")]
    LifecycleInfrastructure(LifecycleInfrastructureError),
    #[error("Failed to interact with the repository during back-up & restore: {0}")]
    BackupRepository(BackupRepositoryError),
}

impl App {
    pub async fn backup<E, LI, R>(
        self,
        app_name: &AppName,
        lifecycle_infrastructure: &mut LI,
        repository: &mut R,
    ) -> Result<BackedUpApp<E>, AppLifeCycleError<LI::Error, R::Error>>
    where
        E: Clone + serde::Serialize,
        LI: LifecycleInfrastructure<BackedUpElement = E>,
        R: BackupRepository<BackedUpElement = E>,
    {
        let back_up_elements = lifecycle_infrastructure
            .snapshot_for_backup(app_name)
            .await
            .map_err(AppLifeCycleError::LifecycleInfrastructure)?
            .ok_or(AppLifeCycleError::NoBackUpSnapshotAvailable)?;

        if back_up_elements.is_empty() {
            return Err(AppLifeCycleError::NoBackUpSnapshotAvailable);
        }

        let backed_up_elements = repository
            .store_elements_for_backup(app_name, back_up_elements)
            .await
            .map_err(AppLifeCycleError::BackupRepository)?;

        // TODO: what if this part errors out partially?
        lifecycle_infrastructure
            .suspend_backed_up_elements(app_name, &backed_up_elements)
            .await
            .map_err(AppLifeCycleError::LifecycleInfrastructure)?;

        BackedUpApp::with_app_and_elements(self, backed_up_elements).map_err(|e| match e {
            BackedUpAppError::NoElements => {
                unreachable!("no elements should have been checked above")
            }
        })
    }
}

impl<E> BackedUpApp<E> {
    pub async fn restore<LI, R>(
        self,
        app_name: &AppName,
        lifecycle_infrastructure: &mut LI,
        repository: &mut R,
    ) -> Result<App, AppLifeCycleError<LI::Error, R::Error>>
    where
        LI: LifecycleInfrastructure<BackedUpElement = E>,
        R: BackupRepository<BackedUpElement = E>,
    {
        let (app, opaque_elements) = self.into_app_and_elements();

        let restored_elements = lifecycle_infrastructure
            .restore_from_backup(app_name, opaque_elements)
            .await
            .map_err(AppLifeCycleError::LifecycleInfrastructure)?;

        repository
            .delete_restored_elements(app_name, restored_elements)
            .await
            .map_err(AppLifeCycleError::BackupRepository)?;

        Ok(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::{mock, predicate};

    mock! {
        pub LifecycleInfra {}
        #[async_trait::async_trait]
        impl LifecycleInfrastructure for LifecycleInfra {
            type BackedUpElement = ();
            type Error = &'static str;

            async fn snapshot_for_backup(
                &self,
                app_name: &AppName,
            ) -> Result<Option<Vec<()>>, &'static str>;

            async fn suspend_backed_up_elements(
                &mut self,
                app_name: &AppName,
                backed_up_elements: &[()],
            ) -> Result<(), &'static str>;

            async fn restore_from_backup(
                &mut self,
                app_name: &AppName,
                backed_up_elements: Vec<()>,
            ) -> Result<Vec<()>, &'static str>;
        }
    }

    mock! {
        pub Repo {}
        #[async_trait::async_trait]
        impl BackupRepository for Repo {
            type BackedUpElement = ();
            type Error = &'static str;

            async fn store_elements_for_backup(
                &mut self,
                app_name: &AppName,
                backed_up_elements: Vec<()>,
            ) -> Result<Vec<()>, &'static str>;

            async fn delete_restored_elements(
                &mut self,
                app_name: &AppName,
                restored_elements: Vec<()>,
            ) -> Result<(), &'static str>;
        }
    }

    mod restore {
        use super::*;

        #[tokio::test]
        async fn restore() -> anyhow::Result<()> {
            let backed_up_app = BackedUpApp::with_app_and_elements(
                App::new(Default::default(), Default::default(), None, None),
                vec![()],
            )?;

            let mut lifecycle_infrastructure = MockLifecycleInfra::new();
            lifecycle_infrastructure
                .expect_restore_from_backup()
                .with(predicate::eq(AppName::master()), predicate::eq(vec![()]))
                .return_const(Ok(vec![()]));
            let mut repository = MockRepo::new();
            repository
                .expect_delete_restored_elements()
                .with(predicate::eq(AppName::master()), predicate::eq(vec![()]))
                .return_const(Ok(()));

            let app = backed_up_app
                .restore(
                    &AppName::master(),
                    &mut lifecycle_infrastructure,
                    &mut repository,
                )
                .await?;

            assert_eq!(
                app,
                App::new(Default::default(), Default::default(), None, None),
            );

            Ok(())
        }

        #[tokio::test]
        async fn fail_to_restore_due_to_infra_error() -> anyhow::Result<()> {
            let backed_up_app = BackedUpApp::with_app_and_elements(
                App::new(Default::default(), Default::default(), None, None),
                vec![()],
            )?;

            let mut lifecycle_infrastructure = MockLifecycleInfra::new();
            lifecycle_infrastructure
                .expect_restore_from_backup()
                .with(predicate::eq(AppName::master()), predicate::eq(vec![()]))
                .return_const(Err("restore error"));
            let mut repository = MockRepo::new();

            let result = backed_up_app
                .restore(
                    &AppName::master(),
                    &mut lifecycle_infrastructure,
                    &mut repository,
                )
                .await;

            assert_eq!(
                result,
                Err(AppLifeCycleError::LifecycleInfrastructure("restore error"))
            );

            Ok(())
        }

        #[tokio::test]
        async fn fail_to_restore_due_to_repository_error() -> anyhow::Result<()> {
            let backed_up_app = BackedUpApp::with_app_and_elements(
                App::new(Default::default(), Default::default(), None, None),
                vec![()],
            )?;

            let mut lifecycle_infrastructure = MockLifecycleInfra::new();
            lifecycle_infrastructure
                .expect_restore_from_backup()
                .with(predicate::eq(AppName::master()), predicate::eq(vec![()]))
                .return_const(Ok(vec![()]));
            let mut repository = MockRepo::new();
            repository
                .expect_delete_restored_elements()
                .with(predicate::eq(AppName::master()), predicate::eq(vec![()]))
                .return_const(Err("sql error"));

            let result = backed_up_app
                .restore(
                    &AppName::master(),
                    &mut lifecycle_infrastructure,
                    &mut repository,
                )
                .await;

            assert_eq!(
                result,
                Err(AppLifeCycleError::BackupRepository("sql error"))
            );

            Ok(())
        }
    }

    mod backup {
        use super::*;

        #[tokio::test]
        async fn backup() -> anyhow::Result<()> {
            let app = App::new(Default::default(), Default::default(), None, None);

            let mut lifecycle_infrastructure = MockLifecycleInfra::new();
            lifecycle_infrastructure
                .expect_snapshot_for_backup()
                .with(predicate::eq(AppName::master()))
                .return_const(Ok(Some(vec![()])));
            let mut repository = MockRepo::new();
            repository
                .expect_store_elements_for_backup()
                .with(predicate::eq(AppName::master()), predicate::always())
                .return_const(Ok(vec![()]));
            lifecycle_infrastructure
                .expect_suspend_backed_up_elements()
                .with(predicate::eq(AppName::master()), predicate::eq(vec![()]))
                .return_const(Ok(()));

            let backed_up_app = app
                .backup(
                    &AppName::master(),
                    &mut lifecycle_infrastructure,
                    &mut repository,
                )
                .await?;

            assert_eq!(
                backed_up_app,
                BackedUpApp::with_app_and_elements(
                    App::new(Default::default(), Default::default(), None, None),
                    vec![()],
                )?,
            );

            Ok(())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(Some(Vec::new()))]
        #[case(None)]
        async fn fail_to_backup_due_to_no_elements(#[case] snapshot_for_backup: Option<Vec<()>>) {
            let app = App::new(Default::default(), Default::default(), None, None);

            let mut lifecycle_infrastructure = MockLifecycleInfra::new();
            lifecycle_infrastructure
                .expect_snapshot_for_backup()
                .with(predicate::eq(AppName::master()))
                .return_const(Ok(snapshot_for_backup));
            let mut repository = MockRepo::new();

            let result = app
                .backup(
                    &AppName::master(),
                    &mut lifecycle_infrastructure,
                    &mut repository,
                )
                .await;

            assert_eq!(result, Err(AppLifeCycleError::NoBackUpSnapshotAvailable));
        }
        #[tokio::test]
        async fn fail_to_backup_due_to_snapshot_error() {
            let app = App::new(Default::default(), Default::default(), None, None);

            let mut lifecycle_infrastructure = MockLifecycleInfra::new();
            lifecycle_infrastructure
                .expect_snapshot_for_backup()
                .with(predicate::eq(AppName::master()))
                .return_const(Err("snapshot error"));
            let mut repository = MockRepo::new();

            let result = app
                .backup(
                    &AppName::master(),
                    &mut lifecycle_infrastructure,
                    &mut repository,
                )
                .await;

            assert_eq!(
                result,
                Err(AppLifeCycleError::LifecycleInfrastructure("snapshot error"))
            );
        }

        #[tokio::test]
        async fn fail_to_backup_due_to_failed_repository() {
            let app = App::new(Default::default(), Default::default(), None, None);

            let mut lifecycle_infrastructure = MockLifecycleInfra::new();
            lifecycle_infrastructure
                .expect_snapshot_for_backup()
                .with(predicate::eq(AppName::master()))
                .return_const(Ok(Some(vec![()])));
            let mut repository = MockRepo::new();
            repository
                .expect_store_elements_for_backup()
                .with(predicate::eq(AppName::master()), predicate::always())
                .return_const(Err("sql error"));

            let result = app
                .backup(
                    &AppName::master(),
                    &mut lifecycle_infrastructure,
                    &mut repository,
                )
                .await;

            assert_eq!(
                result,
                Err(AppLifeCycleError::BackupRepository("sql error"))
            );
        }

        #[tokio::test]
        async fn fail_to_backup_due_to_failed_suspend_on_infra() {
            let app = App::new(Default::default(), Default::default(), None, None);

            let mut lifecycle_infrastructure = MockLifecycleInfra::new();
            lifecycle_infrastructure
                .expect_snapshot_for_backup()
                .with(predicate::eq(AppName::master()))
                .return_const(Ok(Some(vec![()])));
            let mut repository = MockRepo::new();
            repository
                .expect_store_elements_for_backup()
                .with(predicate::eq(AppName::master()), predicate::always())
                .return_const(Ok(vec![()]));
            lifecycle_infrastructure
                .expect_suspend_backed_up_elements()
                .with(predicate::eq(AppName::master()), predicate::eq(vec![()]))
                .return_const(Err("suspend error"));

            let result = app
                .backup(
                    &AppName::master(),
                    &mut lifecycle_infrastructure,
                    &mut repository,
                )
                .await;

            assert_eq!(
                result,
                Err(AppLifeCycleError::LifecycleInfrastructure("suspend error"))
            );
        }
    }
}
