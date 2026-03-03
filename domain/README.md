# PREvant Domain Objects

This crate provides the domain objects that shall provide the core application
logic. Given the example below, the APIs provided in this crate model the
workflow that is described in [the PREvant paper, see Section
4](http://dx.doi.org/10.4230/OASIcs.Microservices.2017-2019.5).

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
