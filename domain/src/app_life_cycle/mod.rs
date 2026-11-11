//! Based on [running](crate::app_instance::App) or [backed-up
//! applications](crate::app_instance::BackedUpApp) this module provide state transition operations
//! between the different types of applications.
use crate::{
    AppName,
    app_instance::{App, BackedUpApp},
};
use std::marker::PhantomData;

#[async_trait::async_trait]
pub trait LifecycleInfrastructure {
    type BackupPayload;
    type Error;

    async fn snapshot_for_backup(
        &self,
        app_name: &AppName,
    ) -> Result<Option<Self::BackupPayload>, Self::Error>;

    async fn suspend_backed_up_elements(
        &self,
        app_name: &AppName,
        backed_up_payload: Self::BackupPayload,
    ) -> Result<(), Self::Error>;

    async fn restore_from_backup(
        &self,
        app_name: &AppName,
        backed_up_payload: Self::BackupPayload,
    ) -> Result<Self::BackupPayload, Self::Error>;
}

#[async_trait::async_trait]
pub trait BackupRepository {
    type BackupPayload;
    type Error;

    async fn store_elements_for_backup<ID>(
        &self,
        app_name: &AppName,
        backed_up_payload: Self::BackupPayload,
    ) -> Result<ID, Self::Error>;

    async fn delete_restored_elements(
        &self,
        app_name: &AppName,
        backed_up_payload: Self::BackupPayload,
    ) -> Result<(), Self::Error>;
}

pub struct AppBackupSnapshot<BackupPayload> {
    pub app_name: AppName,
    pub app: App,
    payload: BackupPayload,
    marker: PhantomData<()>,
}

impl<P> AppBackupSnapshot<P> {
    pub async fn store<ID, R>(self, repository: &R) -> Result<ID, R::Error>
    where
        R: BackupRepository<BackupPayload = P>,
    {
        repository
            .store_elements_for_backup(&self.app_name, self.payload)
            .await
    }
    pub async fn delete<R>(self, repository: &R) -> Result<(), R::Error>
    where
        R: BackupRepository<BackupPayload = P>,
    {
        repository
            .delete_restored_elements(&self.app_name, self.payload)
            .await
    }
}

impl<P> AppBackupSnapshot<P>
where
    P: Clone,
{
    pub async fn suspend_snapshot<LI>(
        self,
        lifecycle_infrastructure: &LI,
    ) -> Result<BackedUpApp<P>, LI::Error>
    where
        LI: LifecycleInfrastructure<BackupPayload = P>,
    {
        lifecycle_infrastructure
            .suspend_backed_up_elements(&self.app_name, self.payload.clone())
            .await?;

        Ok(BackedUpApp::with_app_and_payload(self.app, self.payload))
    }
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
    pub async fn snapshot<P, LI>(
        self,
        app_name: AppName,
        lifecycle_infrastructure: &LI,
    ) -> Result<Option<AppBackupSnapshot<P>>, LI::Error>
    where
        LI: LifecycleInfrastructure<BackupPayload = P>,
    {
        let payload = lifecycle_infrastructure
            .snapshot_for_backup(&app_name)
            .await?;

        Ok(payload.map(|payload| AppBackupSnapshot {
            app_name,
            app: self,
            payload,
            marker: PhantomData,
        }))
    }
}

impl<E> BackedUpApp<E>
where
    E: Clone,
{
    pub async fn restore<LI>(
        self,
        app_name: AppName,
        lifecycle_infrastructure: &LI,
    ) -> Result<AppBackupSnapshot<E>, LI::Error>
    where
        LI: LifecycleInfrastructure<BackupPayload = E>,
    {
        let (app, payload) = self.into_app_and_payload();

        let payload = lifecycle_infrastructure
            .restore_from_backup(&app_name, payload)
            .await?;

        Ok(AppBackupSnapshot {
            app_name,
            app,
            payload,
            marker: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    // use super::*;
    // use mockall::{mock, predicate};
    //
    // mock! {
    //     pub LifecycleInfra {}
    //     #[async_trait::async_trait]
    //     impl LifecycleInfrastructure for LifecycleInfra {
    //         type BackupPayload = ();
    //         type Error = &'static str;
    //
    //         async fn snapshot_for_backup(
    //             &self,
    //             app_name: &AppName,
    //         ) -> Result<Option<()>, &'static str>;
    //
    //         async fn suspend_backed_up_elements(
    //             &mut self,
    //             app_name: &AppName,
    //             backed_up_payload: &(),
    //         ) -> Result<(), &'static str>;
    //
    //         async fn restore_from_backup(
    //             &mut self,
    //             app_name: &AppName,
    //             backed_up_payload: (),
    //         ) -> Result<(), &'static str>;
    //     }
    // }
    //
    // mock! {
    //     pub Repo {}
    //     #[async_trait::async_trait]
    //     impl BackupRepository for Repo {
    //         type BackupPayload = ();
    //         type Error = &'static str;
    //
    //         async fn store_elements_for_backup(
    //             &mut self,
    //             app_name: &AppName,
    //             backed_up_payload: (),
    //         ) -> Result<(), &'static str>;
    //
    //         async fn delete_restored_elements(
    //             &mut self,
    //             app_name: &AppName,
    //             restored_payload: (),
    //         ) -> Result<(), &'static str>;
    //     }
    // }
    //
    // mod restore {
    //     use super::*;
    //
    //     #[tokio::test]
    //     async fn restore() -> anyhow::Result<()> {
    //         let backed_up_app = BackedUpApp::with_app_and_payload(
    //             App::new(Default::default(), Default::default(), None, None),
    //             (),
    //         );
    //
    //         let mut lifecycle_infrastructure = MockLifecycleInfra::new();
    //         lifecycle_infrastructure
    //             .expect_restore_from_backup()
    //             .with(predicate::eq(AppName::master()), predicate::eq(()))
    //             .return_const(Ok(()));
    //         let mut repository = MockRepo::new();
    //         repository
    //             .expect_delete_restored_elements()
    //             .with(predicate::eq(AppName::master()), predicate::eq(()))
    //             .return_const(Ok(()));
    //
    //         let app = backed_up_app
    //             .restore(
    //                 &AppName::master(),
    //                 &mut lifecycle_infrastructure,
    //                 &mut repository,
    //             )
    //             .await?;
    //
    //         assert_eq!(
    //             app,
    //             App::new(Default::default(), Default::default(), None, None),
    //         );
    //
    //         Ok(())
    //     }
    //
    //     #[tokio::test]
    //     async fn fail_to_restore_due_to_infra_error() -> anyhow::Result<()> {
    //         let backed_up_app = BackedUpApp::with_app_and_payload(
    //             App::new(Default::default(), Default::default(), None, None),
    //             (),
    //         );
    //
    //         let mut lifecycle_infrastructure = MockLifecycleInfra::new();
    //         lifecycle_infrastructure
    //             .expect_restore_from_backup()
    //             .with(predicate::eq(AppName::master()), predicate::eq(()))
    //             .return_const(Err("restore error"));
    //         let mut repository = MockRepo::new();
    //
    //         let result = backed_up_app
    //             .restore(
    //                 &AppName::master(),
    //                 &mut lifecycle_infrastructure,
    //                 &mut repository,
    //             )
    //             .await;
    //
    //         assert_eq!(
    //             result,
    //             Err(AppLifeCycleError::LifecycleInfrastructure("restore error"))
    //         );
    //
    //         Ok(())
    //     }
    //
    //     #[tokio::test]
    //     async fn fail_to_restore_due_to_repository_error() -> anyhow::Result<()> {
    //         let backed_up_app = BackedUpApp::with_app_and_payload(
    //             App::new(Default::default(), Default::default(), None, None),
    //             (),
    //         );
    //
    //         let mut lifecycle_infrastructure = MockLifecycleInfra::new();
    //         lifecycle_infrastructure
    //             .expect_restore_from_backup()
    //             .with(predicate::eq(AppName::master()), predicate::eq(()))
    //             .return_const(Ok(()));
    //         let mut repository = MockRepo::new();
    //         repository
    //             .expect_delete_restored_elements()
    //             .with(predicate::eq(AppName::master()), predicate::eq(()))
    //             .return_const(Err("sql error"));
    //
    //         let result = backed_up_app
    //             .restore(
    //                 &AppName::master(),
    //                 &mut lifecycle_infrastructure,
    //                 &mut repository,
    //             )
    //             .await;
    //
    //         assert_eq!(
    //             result,
    //             Err(AppLifeCycleError::BackupRepository("sql error"))
    //         );
    //
    //         Ok(())
    //     }
    // }
    //
    // mod backup {
    //     use super::*;
    //
    //     #[tokio::test]
    //     async fn backup() -> anyhow::Result<()> {
    //         let app = App::new(Default::default(), Default::default(), None, None);
    //
    //         let mut lifecycle_infrastructure = MockLifecycleInfra::new();
    //         lifecycle_infrastructure
    //             .expect_snapshot_for_backup()
    //             .with(predicate::eq(AppName::master()))
    //             .return_const(Ok(Some(())));
    //         let mut repository = MockRepo::new();
    //         repository
    //             .expect_store_elements_for_backup()
    //             .with(predicate::eq(AppName::master()), predicate::always())
    //             .return_const(Ok(()));
    //         lifecycle_infrastructure
    //             .expect_suspend_backed_up_elements()
    //             .with(predicate::eq(AppName::master()), predicate::eq(()))
    //             .return_const(Ok(()));
    //
    //         let backed_up_app = app
    //             .backup(
    //                 &AppName::master(),
    //                 &mut lifecycle_infrastructure,
    //                 &mut repository,
    //             )
    //             .await?;
    //
    //         assert_eq!(
    //             backed_up_app,
    //             BackedUpApp::with_app_and_payload(
    //                 App::new(Default::default(), Default::default(), None, None),
    //                 (),
    //             ),
    //         );
    //
    //         Ok(())
    //     }
    //
    //     #[tokio::test]
    //     async fn fail_to_backup_due_to_no_elements() {
    //         let app = App::new(Default::default(), Default::default(), None, None);
    //
    //         let mut lifecycle_infrastructure = MockLifecycleInfra::new();
    //         lifecycle_infrastructure
    //             .expect_snapshot_for_backup()
    //             .with(predicate::eq(AppName::master()))
    //             .return_const(Ok(None));
    //         let mut repository = MockRepo::new();
    //
    //         let result = app
    //             .backup(
    //                 &AppName::master(),
    //                 &mut lifecycle_infrastructure,
    //                 &mut repository,
    //             )
    //             .await;
    //
    //         assert_eq!(result, Err(AppLifeCycleError::NoBackUpSnapshotAvailable));
    //     }
    //     #[tokio::test]
    //     async fn fail_to_backup_due_to_snapshot_error() {
    //         let app = App::new(Default::default(), Default::default(), None, None);
    //
    //         let mut lifecycle_infrastructure = MockLifecycleInfra::new();
    //         lifecycle_infrastructure
    //             .expect_snapshot_for_backup()
    //             .with(predicate::eq(AppName::master()))
    //             .return_const(Err("snapshot error"));
    //         let mut repository = MockRepo::new();
    //
    //         let result = app
    //             .backup(
    //                 &AppName::master(),
    //                 &mut lifecycle_infrastructure,
    //                 &mut repository,
    //             )
    //             .await;
    //
    //         assert_eq!(
    //             result,
    //             Err(AppLifeCycleError::LifecycleInfrastructure("snapshot error"))
    //         );
    //     }
    //
    //     #[tokio::test]
    //     async fn fail_to_backup_due_to_failed_repository() {
    //         let app = App::new(Default::default(), Default::default(), None, None);
    //
    //         let mut lifecycle_infrastructure = MockLifecycleInfra::new();
    //         lifecycle_infrastructure
    //             .expect_snapshot_for_backup()
    //             .with(predicate::eq(AppName::master()))
    //             .return_const(Ok(Some(())));
    //         let mut repository = MockRepo::new();
    //         repository
    //             .expect_store_elements_for_backup()
    //             .with(predicate::eq(AppName::master()), predicate::always())
    //             .return_const(Err("sql error"));
    //
    //         let result = app
    //             .backup(
    //                 &AppName::master(),
    //                 &mut lifecycle_infrastructure,
    //                 &mut repository,
    //             )
    //             .await;
    //
    //         assert_eq!(
    //             result,
    //             Err(AppLifeCycleError::BackupRepository("sql error"))
    //         );
    //     }
    //
    //     #[tokio::test]
    //     async fn fail_to_backup_due_to_failed_suspend_on_infra() {
    //         let app = App::new(Default::default(), Default::default(), None, None);
    //
    //         let mut lifecycle_infrastructure = MockLifecycleInfra::new();
    //         lifecycle_infrastructure
    //             .expect_snapshot_for_backup()
    //             .with(predicate::eq(AppName::master()))
    //             .return_const(Ok(Some(())));
    //         let mut repository = MockRepo::new();
    //         repository
    //             .expect_store_elements_for_backup()
    //             .with(predicate::eq(AppName::master()), predicate::always())
    //             .return_const(Ok(()));
    //         lifecycle_infrastructure
    //             .expect_suspend_backed_up_elements()
    //             .with(predicate::eq(AppName::master()), predicate::eq(()))
    //             .return_const(Err("suspend error"));
    //
    //         let result = app
    //             .backup(
    //                 &AppName::master(),
    //                 &mut lifecycle_infrastructure,
    //                 &mut repository,
    //             )
    //             .await;
    //
    //         assert_eq!(
    //             result,
    //             Err(AppLifeCycleError::LifecycleInfrastructure("suspend error"))
    //         );
    //     }
    // }
}
