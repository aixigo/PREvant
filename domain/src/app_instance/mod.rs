//! Based on the module [`app_deployment`](`crate::app_deployment`), that models what has to be
//! deployed onto the infrastructure, this module provides domain objects that describe a target
//! deployment of an application.

use crate::AppName;
pub use app::{
    App, AppWithHostMeta, BackedUpApp, BackedUpAppError, Service, ServiceStatus,
    ServiceWithHostMeta,
};
use either::Either;
pub use status::{AppStatus, ContainerType, ContainerTypeParseError};
use std::collections::HashMap;
pub use web_host_meta::WebHostMeta;

mod app;
mod status;
mod web_host_meta;

#[async_trait::async_trait]
pub trait RuntimeInfrastructure {
    type BackedUpElement: Clone;
    type Error;

    /// Returns a `map` of `app-name` and the details of [the deployed applications](`App`) (or
    /// [backed up application](`BackedUpApp`)).
    async fn fetch_apps(
        &self,
    ) -> Result<HashMap<AppName, Either<App, BackedUpApp<Self::BackedUpElement>>>, Self::Error>;

    /// Fetches the [`App`] (or [`BackedUpApp`]) for a particular [`AppName`].
    async fn fetch_app(
        &self,
        app_name: AppName,
    ) -> Result<Option<Either<App, BackedUpApp<Self::BackedUpElement>>>, Self::Error>;
}

#[async_trait::async_trait]
impl<F, B, E> RuntimeInfrastructure for F
where
    F: Fn() -> Result<HashMap<AppName, Either<App, BackedUpApp<B>>>, E> + Send + Sync,
    B: Clone,
{
    type BackedUpElement = B;
    type Error = E;

    async fn fetch_apps(
        &self,
    ) -> Result<HashMap<AppName, Either<App, BackedUpApp<Self::BackedUpElement>>>, Self::Error>
    {
        (self)()
    }

    async fn fetch_app(
        &self,
        app_name: AppName,
    ) -> Result<Option<Either<App, BackedUpApp<Self::BackedUpElement>>>, Self::Error> {
        Ok(self.fetch_apps().await?.remove(&app_name))
    }
}
