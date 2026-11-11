# PREvant Domain Objects

This crate provides the domain objects that shall provide the core application
logic.

## Application Creation

Given the example below, the APIs provided in [`app_deployment`] model the
workflow that is described in [the PREvant paper, see Section
4](http://dx.doi.org/10.4230/OASIcs.Microservices.2017-2019.5). The main entry
for building deployments is
[`AppDeploymentBuilder`](app_deployment::AppDeploymentBuilder).

```rust
use domain::{AppName, Image, blueprint_service};
use domain::app_deployment::AppDeploymentBuilder;
# use std::{collections::HashMap, str::FromStr};
#
# let runtime = tokio::runtime::Runtime::new().unwrap();
# runtime.block_on(async {
#
// Create the objects that are provided by a person using PREvant.
// This example provides an example configuration that deploys a
// wordpress blog on the PREvant infrastructure.
let app_name = AppName::from_str("latest").unwrap();
let configs = vec![
     blueprint_service!(
         "db",
         "mariadb",
         env = (
             "MARIADB_ROOT_PASSWORD" => "example",
             "MARIADB_USER" => "example-user",
             "MARIADB_PASSWORD" => "my_cool_secret",
             "MARIADB_DATABASE" => "example-database"
         )
     ),
     blueprint_service!(
         "blog",
         "wordpress",
         env = (
             "WORDPRESS_DB_HOST" => "db",
             "WORDPRESS_DB_USER" => "example-user",
             "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
             "WORDPRESS_DB_NAME" => "example-database"
         )
     )
];

let deployment_unit = AppDeploymentBuilder::init(app_name, configs, None)
    .with_static_companions(std::iter::empty())
    .resolve_apps::<anyhow::Error, _>(|app_name| {
        // The responsibility of this closure is to fetch the information of a
        // running application that is running under the given app_name
        Ok::<_, anyhow::Error>(None)
    })
    .await.unwrap()
    .resolve_image_manifests::<anyhow::Error, _>(async |images|
        // The responsibility of this closure is to fetch the images' manifests
        // from the OCI/Docker registries
        Ok(HashMap::new())
    )
    .await.unwrap()
    .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
    .await.unwrap()
    // The compose call now follows the rules that are described in the PREvant
    // paper, such as making sure that the replication logic applies.
    .finish().unwrap();

// use deployment_unit to create or update the services on the backend
// infrastructure, e.g. Docker or Kubernetes
assert_eq!(deployment_unit.services.len(), 2);
# })
```

## Application Life Cycle

A [running application](`app_instance::RuntimeInfrastructure`) can be
transitioned into different states.

### Moving from running to backed-up

```rust
use domain::AppName;
use domain::app_instance::{App, BackedUpApp, RuntimeInfrastructure};
use domain::app_life_cycle::{BackupRepository, LifecycleInfrastructure};
# use either::Either;
# use std::convert::Infallible;
# use std::{collections::HashMap, str::FromStr};
#
# let runtime = tokio::runtime::Runtime::new().unwrap();
# runtime.block_on(async {

type BackupPayload = // …
# ();

// Create an instance of `RuntimeInfrastructure` to fetch applications from the
// infrastructure (here it is mock).
let runtime_infrastructure: Box<dyn RuntimeInfrastructure<BackupPayload = BackupPayload, Error = Infallible>> = Box::new(|| {
   // simulate the fetching services, … from the infra
#  Ok(HashMap::from([(AppName::master(), Either::Left(App::new(
#     Vec::new(), Default::default(), None, None,
#  )))]))
});

let Some(Either::Left(app)) = runtime_infrastructure.fetch_app(AppName::master()).await.unwrap() else {
   # panic!()
};

/// A fake implementation for storing the applications infrastructure payload,
/// such a Kubernetes manifests.
struct NoOpBackupRepository;
#[async_trait::async_trait]
impl BackupRepository for NoOpBackupRepository {
    // Implement the trait
    type BackupPayload = BackupPayload;
    type Error = Infallible;
#
#    async fn store_elements_for_backup(
#        &mut self,
#        app_name: &AppName,
#        backed_up_payload: Self::BackupPayload,
#    ) -> Result<Self::BackupPayload, Self::Error> {
#      Ok(backed_up_payload)
#    }
#
#    async fn delete_restored_elements(
#        &mut self,
#        app_name: &AppName,
#        backed_payload: Self::BackupPayload,
#    ) -> Result<(), Self::Error> {
#      Ok(())
#    }
}

/// A fake implementation for snapshoting, deleting and restoring the
/// applications infrastructure payload, such a Kubernetes manifests.
struct NoOpLifecycleInfrastructure;
#[async_trait::async_trait]
impl LifecycleInfrastructure for NoOpLifecycleInfrastructure {
    // Implement the trait
    type BackupPayload = BackupPayload;
    type Error = Infallible;
#
#    async fn snapshot_for_backup(
#        &self,
#        app_name: &AppName,
#    ) -> Result<Option<Self::BackupPayload>, Self::Error> {
#       Ok(Some(()))
#    }
#
#    async fn suspend_backed_up_elements(
#        &mut self,
#        app_name: &AppName,
#        backed_up_elements: &Self::BackupPayload,
#    ) -> Result<(), Self::Error> {
#      Ok(())
#    }
#
#    async fn restore_from_backup(
#        &mut self,
#        app_name: &AppName,
#        backed_up_payload: Self::BackupPayload,
#    ) -> Result<Self::BackupPayload, Self::Error> {
#      Ok(backed_up_payload)
#    }
}

let mut lifecycle_infrastructure = NoOpLifecycleInfrastructure;
let mut backup_repository = NoOpBackupRepository;

// Perform the backup with the aid of infrastructure and repository.
let backed_up_app: BackedUpApp<BackupPayload> = app.backup(
   &AppName::master(),
   &mut lifecycle_infrastructure,
   &mut backup_repository
).await.unwrap();
# })
```

As the [`app_instance::RuntimeInfrastructure`] may return a [running
application](`app_instance::App`) or a [backed-up
application](`app_instance::BackedUpApp`) the surrounding use case must deal
with the cases.

```rust
use domain::AppName;
use domain::app_instance::{App, RuntimeInfrastructure};
use domain::app_life_cycle::{BackupRepository, LifecycleInfrastructure};
# use either::Either;
# use std::convert::Infallible;
# use std::{collections::HashMap, str::FromStr};
#
# let runtime = tokio::runtime::Runtime::new().unwrap();
# runtime.block_on(async {

let runtime_infrastructure: Box<dyn RuntimeInfrastructure<BackupPayload = (), Error = Infallible>> =
# Box::new(|| {
#   Ok(HashMap::from([(AppName::master(), Either::Left(App::new(
#      Vec::new(), Default::default(), None, None,
#   )))]))
# });

match runtime_infrastructure.fetch_app(AppName::master()).await.unwrap() {
   Some(Either::Left(app)) => {
      // ready to be back up the application
   }
   Some(Either::Right(backed_up_app)) => {
      // already backed-up, cannot backup again.
   }
#   None => {}
};
# })
```

### Moving from backed-up to running

```rust
use domain::AppName;
use domain::app_instance::{App, BackedUpApp, RuntimeInfrastructure};
use domain::app_life_cycle::{BackupRepository, LifecycleInfrastructure};
# use either::Either;
# use std::convert::Infallible;
# use std::{collections::HashMap, str::FromStr};
#
# let runtime = tokio::runtime::Runtime::new().unwrap();
# runtime.block_on(async {

// Create an instance of `RuntimeInfrastructure` to fetch applications from the
// infrastructure (here it is mock).
let runtime_infrastructure: Box<dyn RuntimeInfrastructure<BackupPayload = (), Error = Infallible>> = Box::new(|| {
    // simulate the fetching services, … from the infra
#   Ok(HashMap::from([(AppName::master(), Either::Right(BackedUpApp::with_app_and_payload(App::new(
#      Vec::new(), Default::default(), None, None,
#   ), ())))]))
});

let Some(Either::Right(backed_up_app)) = runtime_infrastructure.fetch_app(AppName::master()).await.unwrap() else {
   # panic!()
};

/// A fake implementation for storing the applications infrastructure payload,
/// such a Kubernetes manifests.
struct NoOpBackupRepository;
#[async_trait::async_trait]
impl BackupRepository for NoOpBackupRepository {
    // Implement the trait
#    type BackupPayload = ();
#    type Error = Infallible;
#
#    async fn store_elements_for_backup(
#        &mut self,
#        app_name: &AppName,
#        backed_up_payload: Self::BackupPayload,
#    ) -> Result<Self::BackupPayload, Self::Error> {
#      Ok(backed_up_payload)
#    }
#
#    async fn delete_restored_elements(
#        &mut self,
#        app_name: &AppName,
#        restored_payload: Self::BackupPayload,
#    ) -> Result<(), Self::Error> {
#      Ok(())
#    }
}

/// A fake implementation for snapshoting, deleting and restoring the
/// applications infrastructure payload, such a Kubernetes manifests.
struct NoOpLifecycleInfrastructure;
#[async_trait::async_trait]
impl LifecycleInfrastructure for NoOpLifecycleInfrastructure {
    // Implement the trait
#    type BackupPayload = ();
#    type Error = Infallible;
#
#    async fn snapshot_for_backup(
#        &self,
#        app_name: &AppName,
#    ) -> Result<Option<Self::BackupPayload>, Self::Error> {
#       Ok(Some(()))
#    }
#
#    async fn suspend_backed_up_elements(
#        &mut self,
#        app_name: &AppName,
#        backed_up_elements: &Self::BackupPayload,
#    ) -> Result<(), Self::Error> {
#      Ok(())
#    }
#
#    async fn restore_from_backup(
#        &mut self,
#        app_name: &AppName,
#        backed_up_payload: Self::BackupPayload,
#    ) -> Result<Self::BackupPayload, Self::Error> {
#      Ok(backed_up_payload)
#    }
}

let mut lifecycle_infrastructure = NoOpLifecycleInfrastructure;
let mut backup_repository = NoOpBackupRepository;

// Perform the backup with the aid of infrastructure and repository.
let app: App = backed_up_app.restore(
   &AppName::master(),
   &mut lifecycle_infrastructure,
   &mut backup_repository
).await.unwrap();
# })
```

As the [`app_instance::RuntimeInfrastructure`] may return a [running
application](`app_instance::App`) or a [backed-up
application](`app_instance::BackedUpApp`) the surrounding use case must deal
with the cases.

```rust
use domain::AppName;
use domain::app_instance::{App, RuntimeInfrastructure};
use domain::app_life_cycle::{BackupRepository, LifecycleInfrastructure};
# use either::Either;
# use std::convert::Infallible;
# use std::{collections::HashMap, str::FromStr};
#
# let runtime = tokio::runtime::Runtime::new().unwrap();
# runtime.block_on(async {

let runtime_infrastructure: Box<dyn RuntimeInfrastructure<BackupPayload = (), Error = Infallible>> =
# Box::new(|| {
#   Ok(HashMap::from([(AppName::master(), Either::Left(App::new(
#      Vec::new(), Default::default(), None, None,
#   )))]))
# });

match runtime_infrastructure.fetch_app(AppName::master()).await.unwrap() {
   Some(Either::Left(app)) => {
      // already running, cannot restore again
   }
   Some(Either::Right(backed_up_app)) => {
      // ready to restore the application
   }
#   None => {}
};
# })
```
