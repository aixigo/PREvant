/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2021 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */
mod host_meta_cache;
mod queue;
mod routes;

pub use crate::apps::AppsService as Apps;
pub use crate::apps::AppsServiceError as AppsError;
use crate::config::ReplicateApplicationCondition;
use crate::config::{Config, ConfigError};
use crate::deployment::deployment_unit::DeploymentTemplatingError;
use crate::deployment::deployment_unit::DeploymentUnitBuilder;
use crate::deployment::hooks::HooksError;
use crate::infrastructure::HttpForwarder;
use crate::infrastructure::Infrastructure;
use crate::infrastructure::TraefikIngressRoute;
use crate::models::user_defined_parameters::UserDefinedParameters;
use crate::models::Owner;
use crate::models::{App, AppName, ContainerType, LogChunk, Service, ServiceConfig, ServiceStatus};
use crate::registry::Registry;
use crate::registry::RegistryError;
use chrono::{DateTime, FixedOffset};
use futures::stream::BoxStream;
use futures::StreamExt;
pub use host_meta_cache::new as host_meta_crawling;
pub use host_meta_cache::HostMetaCache;
use log::debug;
use log::error;
use log::trace;
pub use queue::AppProcessingQueue;
pub use queue::AppTaskQueueProducer;
pub use routes::{apps_routes, delete_app_sync, AppV1};
use std::collections::{HashMap, HashSet};
use std::convert::From;
use std::time::Duration;
use tokio::sync::watch::Receiver;

pub struct AppsService {
    config: Config,
    infrastructure: Box<dyn Infrastructure>,
    prevant_base_route: Option<TraefikIngressRoute>,
}

impl AppsService {
    pub fn new(
        config: Config,
        infrastructure: Box<dyn Infrastructure>,
    ) -> Result<AppsService, AppsServiceError> {
        Ok(AppsService {
            config,
            infrastructure,
            prevant_base_route: None,
        })
    }

    pub fn with_base_route(mut self, prevant_base_route: Option<TraefikIngressRoute>) -> Self {
        self.prevant_base_route = prevant_base_route;
        self
    }

    async fn http_forwarder(&self) -> anyhow::Result<Box<dyn HttpForwarder>> {
        self.infrastructure.http_forwarder().await
    }

    pub async fn fetch_service_of_app(
        &self,
        app_name: &AppName,
        service_name: &str,
    ) -> Result<Option<Service>, AppsServiceError> {
        Ok(self
            .infrastructure
            .fetch_app(app_name)
            .await?
            .and_then(|app| {
                let services = app.into_services();
                services
                    .into_iter()
                    .find(|service| service.service_name() == service_name)
            }))
    }

    pub async fn fetch_app_names(&self) -> Result<HashSet<AppName>, AppsServiceError> {
        Ok(self.infrastructure.fetch_app_names().await?)
    }

    /// Analyzes running containers and returns a map of `app-name` with the
    /// corresponding list of `Service`s.
    pub async fn fetch_apps(&self) -> Result<HashMap<AppName, App>, AppsServiceError> {
        Ok(self.infrastructure.fetch_apps().await?)
    }

    /// Provides a [`Receiver`](tokio::sync::watch::Receiver) that notifies about changes of the
    /// list of running [`apps`](AppsService::fetch_apps).
    pub async fn app_updates(&self) -> Receiver<HashMap<AppName, App>> {
        let infrastructure = dyn_clone::clone_box(&*self.infrastructure);
        let (tx, rx) = tokio::sync::watch::channel::<HashMap<AppName, App>>(HashMap::new());

        tokio::spawn(async move {
            loop {
                debug!("Fetching list of apps to send updates.");
                match infrastructure.fetch_apps().await {
                    Ok(apps) => {
                        tx.send_if_modified(move |state| {
                            if &apps != state {
                                debug!("List of apps changed, sending updates.");
                                *state = apps;
                                true
                            } else {
                                false
                            }
                        });
                    }
                    Err(err) => {
                        error!("Cannot crawl apps from infrastructure: {err}");
                    }
                }

                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });

        rx
    }

    async fn configs_and_user_defined_parameters_to_replicate(
        &self,
        services_to_deploy: &[ServiceConfig],
        running_app: &App,
        replicate_from_app_name: &AppName,
    ) -> Result<(Vec<ServiceConfig>, Option<UserDefinedParameters>), AppsServiceError> {
        let running_instances_names = running_app
            .services()
            .iter()
            .filter(|c| c.container_type() == &ContainerType::Instance)
            .map(|c| c.service_name())
            .collect::<HashSet<&String>>();

        let service_names = services_to_deploy
            .iter()
            .map(|c| c.service_name())
            .collect::<HashSet<&String>>();

        let app = self
            .infrastructure
            .fetch_app(replicate_from_app_name)
            .await?
            .unwrap_or_else(App::empty);

        let (services, user_defined_parameters) = app.into_services_and_user_defined_parameters();

        Ok((
            services
                .into_iter()
                .map(|service| service.config)
                .filter(|config| {
                    matches!(
                        config.container_type(),
                        ContainerType::Instance | ContainerType::Replica
                    )
                })
                .filter(|config| !service_names.contains(config.service_name()))
                .filter(|config| !running_instances_names.contains(config.service_name()))
                .map(|config| {
                    let mut replicated_config = config;
                    replicated_config.set_container_type(ContainerType::Replica);
                    replicated_config
                })
                .collect::<Vec<ServiceConfig>>(),
            user_defined_parameters,
        ))
    }

    /// Creates or updates an app to review with the given service configurations.
    ///
    /// The list of given services will be extended with:
    /// - the replications from the running template application (e.g. master)
    /// - the application companions (see README)
    /// - the service companions (see README)
    ///
    /// # Arguments
    /// * `replicate_from` - The application name that is used as a template.
    async fn create_or_update(
        &self,
        app_name: &AppName,
        replicate_from: Option<AppName>,
        service_configs: &[ServiceConfig],
        owners: Vec<Owner>,
        user_defined_parameters: Option<serde_json::Value>,
    ) -> Result<App, AppsServiceError> {
        let user_defined_parameters = match (
            self.config.user_defined_schema_validator(),
            user_defined_parameters,
        ) {
            (None, _) => None,
            (Some(validator), None) => Some(
                UserDefinedParameters::new(serde_json::json!({}), &validator).map_err(|e| {
                    AppsServiceError::InvalidUserDefinedParameters { err: e.to_string() }
                })?,
            ),
            (Some(validator), Some(value)) => {
                Some(UserDefinedParameters::new(value, &validator).map_err(|e| {
                    AppsServiceError::InvalidUserDefinedParameters { err: e.to_string() }
                })?)
            }
        };

        if let Some(app_limit) = self.config.app_limit() {
            let apps = self.fetch_app_names().await?;

            if apps
                .iter()
                // filtering the app_name that is send because otherwise clients wouldn't be able
                // to update an existing application.
                .filter(|existing_app_name| *existing_app_name != app_name)
                .count()
                + 1
                > app_limit
            {
                return Err(AppsError::AppLimitExceeded { limit: app_limit });
            }
        }

        let mut configs = service_configs.to_vec();

        let running_app = self
            .infrastructure
            .fetch_app(app_name)
            .await?
            .unwrap_or_else(App::empty);

        let mut user_defined_parameters = match (
            running_app.user_defined_parameters().clone(),
            user_defined_parameters,
        ) {
            (None, None) => None,
            (None, Some(user_defined_parameters)) => Some(user_defined_parameters),
            (Some(active_user_defined_parameters), None) => Some(active_user_defined_parameters),
            (Some(active_user_defined_parameters), Some(user_defined_parameters)) => {
                Some(active_user_defined_parameters.merge(user_defined_parameters))
            }
        };

        let replicate_from_app_name = match (
            replicate_from.as_ref(),
            &self.config.applications.replication_condition,
        ) {
            (None, ReplicateApplicationCondition::AlwaysFromDefaultApp) => {
                Some(&self.config.applications.default_app)
            }
            (None, ReplicateApplicationCondition::ExplicitlyMentioned) => None,
            (
                Some(replicate_from_app_name),
                ReplicateApplicationCondition::AlwaysFromDefaultApp
                | ReplicateApplicationCondition::ExplicitlyMentioned,
            ) if replicate_from_app_name != app_name => Some(replicate_from_app_name),
            _ => None,
        };

        if let Some(replicate_from_app_name) = replicate_from_app_name {
            let (config_to_replicate, replication_user_defined_parameters) = self
                .configs_and_user_defined_parameters_to_replicate(
                    service_configs,
                    &running_app,
                    &replicate_from_app_name,
                )
                .await?;
            configs.extend(config_to_replicate);

            user_defined_parameters = match (
                replication_user_defined_parameters,
                user_defined_parameters.take(),
            ) {
                (None, None) => None,
                (None, Some(user_defined_parameters)) => Some(user_defined_parameters),
                (Some(active_user_defined_parameters), None) => {
                    Some(active_user_defined_parameters)
                }
                (Some(active_user_defined_parameters), Some(user_defined_parameters)) => {
                    Some(active_user_defined_parameters.merge(user_defined_parameters))
                }
            };
        }

        let (services, mut existing_owners) = running_app.into_services_and_owners();
        existing_owners.extend(owners);
        let configs_for_templating = services
            .into_iter()
            .filter(|service| service.container_type() == &ContainerType::Instance)
            .filter(|service| {
                !service_configs
                    .iter()
                    .any(|c| c.service_name() == service.service_name())
            })
            .map(|service| service.config)
            .collect::<Vec<_>>();

        let deployment_unit_builder = DeploymentUnitBuilder::init(app_name.clone(), configs)
            .extend_with_config(&self.config)
            .extend_with_templating_only_service_configs(configs_for_templating);

        let images = deployment_unit_builder.images();
        let image_infos = Registry::new(&self.config)
            .resolve_image_infos(&images)
            .await?;

        let deployment_unit_builder = deployment_unit_builder
            .extend_with_image_infos(image_infos)
            .with_owners(existing_owners)
            .apply_templating(
                &self.prevant_base_route.as_ref().and_then(|r| r.to_url()),
                user_defined_parameters,
            )?
            .apply_hooks(&self.config)
            .await?;

        let deployment_unit =
            if let Some(base_traefik_ingress_route) = self.prevant_base_route.as_ref() {
                trace!(
                    "The base URL for {app_name} is: {:?}",
                    base_traefik_ingress_route
                        .to_url()
                        .map(|url| url.to_string())
                );
                deployment_unit_builder
                    .apply_base_traefik_ingress_route(base_traefik_ingress_route.clone())
                    .build()
            } else {
                deployment_unit_builder.build()
            };

        let apps = self
            .infrastructure
            .deploy_services(&deployment_unit, &self.config.container_config())
            .await?;

        Ok(apps)
    }

    /// Deletes all services for the given `app_name`.
    async fn delete_app(&self, app_name: &AppName) -> Result<App, AppsServiceError> {
        let app = self.infrastructure.stop_services(app_name).await?;

        if app.is_empty() {
            Err(AppsServiceError::AppNotFound {
                app_name: app_name.clone(),
            })
        } else {
            Ok(app)
        }
    }

    pub async fn stream_logs<'a>(
        &'a self,
        app_name: &'a AppName,
        service_name: &'a str,
        since: &'a Option<DateTime<FixedOffset>>,
        limit: &'a Option<usize>,
    ) -> BoxStream<'a, Result<(DateTime<FixedOffset>, String), anyhow::Error>> {
        self.infrastructure
            .get_logs(app_name, service_name, since, limit, true)
            .await
    }

    pub async fn get_logs<'a>(
        &'a self,
        app_name: &'a AppName,
        service_name: &'a str,
        since: &'a Option<DateTime<FixedOffset>>,
        limit: &'a Option<usize>,
    ) -> Result<Option<LogChunk>, AppsServiceError> {
        let mut log_lines = Vec::new();
        let mut log_stream = self
            .infrastructure
            .get_logs(app_name, service_name, since, limit, false)
            .await;

        while let Some(result) = log_stream.next().await {
            if let Ok(log_line) = result {
                log_lines.push(log_line);
            }
        }

        Ok(Some(LogChunk::from(log_lines)))
    }

    pub async fn change_status(
        &self,
        app_name: &AppName,
        service_name: &str,
        status: ServiceStatus,
    ) -> Result<Option<Service>, AppsServiceError> {
        Ok(self
            .infrastructure
            .change_status(app_name, service_name, status)
            .await?)
    }
}

/// Defines error cases for the [`Apps`](Apps)
#[derive(Debug, Clone, thiserror::Error, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum AppsServiceError {
    #[error("Cannot find app {app_name}.")]
    AppNotFound { app_name: AppName },
    #[error("Cannot create more than {limit} apps")]
    AppLimitExceeded { limit: usize },
    /// Will be used when the service cannot interact correctly with the infrastructure.
    #[error("Cannot interact with infrastructure: {error}")]
    InfrastructureError { error: String },
    /// Will be used if the service configuration cannot be loaded.
    #[error("Invalid configuration: {error}")]
    InvalidServerConfiguration { error: String },
    #[error(
        "Internal template processing issue (please, contact administrator of the system): {error}"
    )]
    TemplatingIssue { error: DeploymentTemplatingError },
    #[error("Unable to resolve information about image: {error}")]
    UnableToResolveImage { error: RegistryError },
    #[error("Cannot apply hook {err}")]
    UnapplicableHook { err: HooksError },
    #[error("User defined payload does not match to the configured value: {err}")]
    InvalidUserDefinedParameters { err: String },
}

impl From<ConfigError> for AppsServiceError {
    fn from(error: ConfigError) -> Self {
        Self::InvalidServerConfiguration {
            error: error.to_string(),
        }
    }
}

impl From<anyhow::Error> for AppsServiceError {
    fn from(error: anyhow::Error) -> Self {
        Self::InfrastructureError {
            error: error.to_string(),
        }
    }
}

impl From<DeploymentTemplatingError> for AppsServiceError {
    fn from(error: DeploymentTemplatingError) -> Self {
        Self::TemplatingIssue { error }
    }
}

impl From<RegistryError> for AppsServiceError {
    fn from(error: RegistryError) -> Self {
        Self::UnableToResolveImage { error }
    }
}

impl From<HooksError> for AppsServiceError {
    fn from(err: HooksError) -> Self {
        Self::UnapplicableHook { err }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::{Dummy, TraefikIngressRoute, TraefikRouterRule};
    use crate::models::{EnvironmentVariable, Owner, State};
    use crate::sc;
    use chrono::Utc;
    use futures::StreamExt;
    use openidconnect::{IssuerUrl, SubjectIdentifier};
    use secstr::SecUtf8;
    use std::hash::Hash;
    use std::io::Write;
    use std::path::PathBuf;
    use std::str::FromStr;
    use tempfile::NamedTempFile;

    macro_rules! config_from_str {
        ( $config_str:expr_2021 ) => {
            toml::from_str::<Config>($config_str).unwrap()
        };
    }

    macro_rules! assert_contains_service {
        ( $services:expr_2021, $service_name:expr_2021, $container_type:expr_2021 ) => {
            assert!(
                $services
                    .iter()
                    .find(|s| s.service_name() == $service_name
                        && s.container_type() == &$container_type)
                    .is_some(),
                "services should contain {:?} with type {:?}",
                $service_name,
                $container_type
            );
        };
    }

    fn config_with_deployment_hook(script: &str) -> (NamedTempFile, Config) {
        let mut hook_file = NamedTempFile::new().unwrap();

        hook_file.write_all(script.as_bytes()).unwrap();

        let config = crate::config_from_str!(&format!(
            r#"
            [hooks]
            deployment = {:?}
            "#,
            hook_file.path()
        ));

        (hook_file, config)
    }

    #[tokio::test]
    async fn should_create_app_for_master() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a")],
            vec![],
            None,
        )
        .await?;

        let deployed_apps = apps.fetch_apps().await?;
        assert_eq!(deployed_apps.len(), 1);
        let app = deployed_apps.get(&AppName::master()).unwrap();
        assert_eq!(app.services().len(), 1);
        assert_contains_service!(app.services(), "service-a", ContainerType::Instance);

        Ok(())
    }

    #[tokio::test]
    async fn replicate_from_master() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            None,
            &vec![sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        let deployed_apps = apps.fetch_apps().await?;

        let app = deployed_apps
            .get(&AppName::from_str("branch").unwrap())
            .unwrap();
        assert_eq!(app.services().len(), 2);
        assert_contains_service!(app.services(), "service-b", ContainerType::Instance);
        assert_contains_service!(app.services(), "service-a", ContainerType::Replica);

        Ok(())
    }


    #[tokio::test]
    async fn replicate_from_default_application_if_specified() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [applications]
            replicationCondition = 'replicate-only-when-requested'
            "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            None,
            &vec![sc!("service-c")],
            vec![],
            None,
        )
        .await?;

        let deployed_apps = apps.fetch_apps().await?;

        let app = deployed_apps
            .get(&AppName::from_str("branch").unwrap())
            .unwrap();
        assert_eq!(app.services().len(), 1);
        assert_contains_service!(app.services(), "service-c", ContainerType::Instance);

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            Some(AppName::master()),
            &vec![sc!("service-c")],
            vec![],
            None,
        )
        .await?;

        let deployed_apps = apps.fetch_apps().await?;

        let app = deployed_apps
            .get(&AppName::from_str("branch").unwrap())
            .unwrap();
        assert_eq!(app.services().len(), 3);
        assert_contains_service!(app.services(), "service-a", ContainerType::Replica);
        assert_contains_service!(app.services(), "service-b", ContainerType::Replica);
        assert_contains_service!(app.services(), "service-c", ContainerType::Instance);

        Ok(())
    }

    #[tokio::test]
    async fn never_replicate_from_default_application() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [applications]
            replicationCondition = 'never'
            "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            Some(AppName::master()),
            &vec![sc!("service-c")],
            vec![],
            None,
        )
        .await?;

        let deployed_apps = apps.fetch_apps().await?;

        let app = deployed_apps
            .get(&AppName::from_str("branch").unwrap())
            .unwrap();
        assert_eq!(app.services().len(), 1);
        assert_contains_service!(app.services(), "service-c", ContainerType::Instance);

        Ok(())
    }

    #[tokio::test]
    async fn override_replicas_from_master() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            None,
            &vec![sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            None,
            &vec![sc!("service-a")],
            vec![],
            None,
        )
        .await?;

        let deployed_apps = apps.fetch_apps().await?;

        let app = deployed_apps
            .get(&AppName::from_str("branch").unwrap())
            .unwrap();
        assert_eq!(app.services().len(), 2);
        assert_contains_service!(app.services(), "service-a", ContainerType::Instance);
        assert_contains_service!(app.services(), "service-b", ContainerType::Instance);

        Ok(())
    }

    #[tokio::test]
    async fn should_create_app_for_master_with_secrets() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [services.mariadb]
            [[services.mariadb.secrets]]
            name = "user"
            data = "SGVsbG8="
            appSelector = "master"
        "#
        );

        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("mariadb")],
            vec![],
            None,
        )
        .await?;

        let app = apps
            .infrastructure
            .fetch_app(&AppName::master())
            .await?
            .unwrap();
        assert_eq!(app.services().iter().count(), 1);

        let config = app
            .into_services()
            .into_iter()
            .next()
            .map(|s| s.config)
            .unwrap();
        let files = config.files().unwrap();
        assert_eq!(
            files.get(&PathBuf::from("/run/secrets/user")).unwrap(),
            &SecUtf8::from("Hello")
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_create_app_for_master_without_secrets_because_of_none_matching_app_selector(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [services.mariadb]
            [[services.mariadb.secrets]]
            name = "user"
            data = "SGVsbG8="
            appSelector = "master"
        "#
        );

        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master-1x").unwrap(),
            None,
            &vec![sc!("mariadb")],
            vec![],
            None,
        )
        .await?;

        let app = apps
            .infrastructure
            .fetch_app(&AppName::from_str("master-1x").unwrap())
            .await?
            .unwrap();
        assert_eq!(app.services().len(), 1);

        let config = app
            .into_services()
            .into_iter()
            .next()
            .map(|s| s.config)
            .unwrap();
        assert_eq!(config.files(), None);

        Ok(())
    }

    #[tokio::test]
    async fn should_collect_log_chunk_from_infrastructure() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::master();

        apps.create_or_update(
            &app_name,
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        let log_chunk = apps
            .get_logs(&app_name, &String::from("service-a"), &None, &Some(100))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            log_chunk.since(),
            &DateTime::parse_from_rfc3339("2019-07-18T07:25:00.000000000Z").unwrap()
        );

        assert_eq!(
            log_chunk.until(),
            &DateTime::parse_from_rfc3339("2019-07-18T07:35:00.000000000Z").unwrap()
        );

        assert_eq!(
            log_chunk.log_lines(),
            r#"Log msg 1 of service-a of app master
Log msg 2 of service-a of app master
Log msg 3 of service-a of app master
"#
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_stream_logs_from_infrastructure() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::from_str("master").unwrap();
        let services = vec![sc!("service-a"), sc!("service-b")];
        apps.create_or_update(&app_name, None, &services, vec![], None)
            .await?;
        for service in services {
            let mut log_stream = apps
                .stream_logs(&app_name, service.service_name(), &None, &None)
                .await;

            assert_eq!(
                log_stream.next().await.unwrap().unwrap(),
                (
                    DateTime::parse_from_rfc3339("2019-07-18T07:25:00.000000000Z").unwrap(),
                    format!(
                        "Log msg 1 of {} of app {app_name}\n",
                        service.service_name()
                    )
                )
            );

            assert_eq!(
                log_stream.next().await.unwrap().unwrap(),
                (
                    DateTime::parse_from_rfc3339("2019-07-18T07:30:00.000000000Z").unwrap(),
                    format!(
                        "Log msg 2 of {} of app {app_name}\n",
                        service.service_name()
                    )
                )
            );

            assert_eq!(
                log_stream.next().await.unwrap().unwrap(),
                (
                    DateTime::parse_from_rfc3339("2019-07-18T07:35:00.000000000Z").unwrap(),
                    format!(
                        "Log msg 3 of {} of app {app_name}\n",
                        service.service_name()
                    )
                )
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn should_deploy_companions() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'keycloak/keycloak:23.0'

            [companions.db]
            serviceName = 'db'
            type = 'service'
            image = 'postgres:16.1'
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::master();
        apps.create_or_update(&app_name, None, &vec![sc!("service-a")], vec![], None)
            .await?;
        let deployed_apps = apps.fetch_apps().await?;

        let app = deployed_apps.get(&app_name).unwrap();
        assert_eq!(app.services().len(), 3);
        assert_contains_service!(
            app.services(),
            "openid",
            ContainerType::ApplicationCompanion
        );
        assert_contains_service!(app.services(), "db", ContainerType::ServiceCompanion);
        assert_contains_service!(app.services(), "service-a", ContainerType::Instance);

        Ok(())
    }

    #[tokio::test]
    async fn should_filter_companions_if_services_to_deploy_contain_same_service_name(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'keycloak/keycloak:23.0'

            [companions.db]
            serviceName = 'db'
            type = 'service'
            image = 'postgres:16.1'
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::master();
        let configs = vec![sc!("openid"), sc!("db")];
        apps.create_or_update(&app_name, None, &configs, vec![], None)
            .await?;
        let deployed_apps = apps.fetch_apps().await?;

        let deployed_app = deployed_apps.get(&app_name).unwrap();
        assert_eq!(deployed_app.services().len(), 2);
        assert_contains_service!(deployed_app.services(), "openid", ContainerType::Instance);
        assert_contains_service!(deployed_app.services(), "db", ContainerType::Instance);

        let openid_configs: Vec<ServiceConfig> = apps
            .infrastructure
            .fetch_app(&app_name)
            .await?
            .map(|app| app.into_services())
            .unwrap()
            .into_iter()
            .filter(|service| service.service_name() == "openid")
            .map(|service| service.config)
            .collect();
        assert_eq!(openid_configs.len(), 1);
        assert_eq!(openid_configs[0].image(), configs[0].image());

        Ok(())
    }

    #[tokio::test]
    async fn should_merge_with_companion_config_if_services_to_deploy_contain_same_service_name(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'keycloak/keycloak:23.0'
            env = [ "VAR_1=abcd", "VAR_2=1234" ]

            [companions.openid.labels]
            'traefik.frontend.rule' = 'PathPrefix:/example.com/openid/;'
            'traefik.frontend.priority' = '20000'
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::master();

        let configs = vec![crate::sc!(
            "openid",
            labels = (),
            env = ("VAR_1" => "efg"),
            files = ()
        )];

        apps.create_or_update(&app_name, None, &configs, vec![], None)
            .await?;

        let deployed_apps = apps.fetch_apps().await?;

        let deployed_app = deployed_apps.get(&app_name).unwrap();
        assert_eq!(deployed_app.services().len(), 1);
        assert_contains_service!(deployed_app.services(), "openid", ContainerType::Instance);

        let openid_configs: Vec<ServiceConfig> = apps
            .infrastructure
            .fetch_app(&app_name)
            .await?
            .map(|app| app.into_services())
            .unwrap()
            .into_iter()
            .filter(|service| service.service_name() == "openid")
            .map(|service| service.config)
            .collect();
        assert_eq!(openid_configs.len(), 1);

        use secstr::SecUtf8;
        let openid_env = openid_configs[0].env().unwrap();
        assert_eq!(
            openid_env.variable("VAR_1"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_1"),
                SecUtf8::from("efg")
            ))
        );
        assert_eq!(
            openid_env.variable("VAR_2"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_2"),
                SecUtf8::from("1234")
            ))
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_include_running_instance_in_templating() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'keycloak/keycloak:23.0'
            env = [ """SERVICES={{~#each services~}}{{name}},{{~/each~}}""" ]
        "#
        );

        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;
        let app_name = AppName::master();

        apps.create_or_update(
            &app_name,
            None,
            &vec![crate::sc!("service-a")],
            vec![],
            None,
        )
        .await?;
        apps.create_or_update(
            &app_name,
            None,
            &vec![crate::sc!("service-b")],
            vec![],
            None,
        )
        .await?;
        apps.create_or_update(
            &app_name,
            None,
            &vec![crate::sc!("service-c")],
            vec![],
            None,
        )
        .await?;

        let mut apps = apps.infrastructure.fetch_apps().await?;
        let openid_config = apps
            .remove(&AppName::master())
            .unwrap()
            .into_services()
            .into_iter()
            .find(|service| service.service_name() == "openid")
            .map(|service| service.config)
            .unwrap();
        let openid_env = openid_config.env().unwrap().get(0).unwrap();

        assert_eq!(
            openid_env.value().unsecure(),
            "service-a,service-b,service-c,"
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_delete_apps() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::master();
        apps.create_or_update(&app_name, None, &vec![sc!("service-a")], vec![], None)
            .await?;
        let deleted_services = apps.delete_app(&app_name).await?;

        assert_eq!(
            deleted_services,
            App::new(
                vec![Service {
                    id: "service-a".to_string(),
                    config: crate::sc!("service-a"),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: Some(
                            DateTime::parse_from_rfc3339("2019-07-18T07:25:00.000000000Z")
                                .unwrap()
                                .with_timezone(&Utc)
                        ),
                    }
                }],
                HashSet::new(),
                None
            )
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_deploy_companions_with_file_mount() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.db1]
            serviceName = 'db1'
            type = 'service'
            image = 'postgres:16.1'

            [companions.db1.volumes]
            '/etc/mysql1/my.cnf' = 'EFGH'

            [companions.db2]
            serviceName = 'db2'
            type = 'service'
            image = 'postgres:16.1'

            [companions.db2.files]
            '/etc/mysql2/my.cnf' = 'ABCD'
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::master();
        let configs = vec![sc!("db1"), sc!("db2")];
        apps.create_or_update(&app_name, None, &configs, vec![], None)
            .await?;
        let deployed_apps = apps.fetch_apps().await?;

        let db_config1: Vec<ServiceConfig> = apps
            .infrastructure
            .fetch_app(&app_name)
            .await?
            .map(|app| app.into_services())
            .unwrap()
            .into_iter()
            .filter(|service| service.service_name() == "db1")
            .map(|service| service.config)
            .collect();

        let db_config2: Vec<ServiceConfig> = apps
            .infrastructure
            .fetch_app(&app_name)
            .await?
            .map(|app| app.into_services())
            .unwrap()
            .into_iter()
            .filter(|service| service.service_name() == "db2")
            .map(|service| service.config)
            .collect();

        let app = deployed_apps.get(&app_name).unwrap();

        assert_eq!(app.services().len(), 2);
        assert_eq!(
            db_config1[0]
                .files()
                .expect("Empty Map")
                .get(&PathBuf::from("/etc/mysql1/my.cnf"))
                .expect("Invalid entry in Map"),
            &SecUtf8::from("EFGH")
        );

        assert_eq!(
            db_config2[0]
                .files()
                .expect("Empty Map")
                .get(&PathBuf::from("/etc/mysql2/my.cnf"))
                .expect("Invalid entry in Map"),
            &SecUtf8::from("ABCD")
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_create_app_with_hooks_applied() -> Result<(), AppsServiceError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            if( appName === 'master' ) {
                return configs.filter( service => service.name !== 'service-b' );
            }
            return configs;
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let app_name = &AppName::master();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            app_name,
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        let deployed_services = apps.fetch_apps().await?;
        let service_names = deployed_services
            .iter()
            .flat_map(|(_, app)| app.services().iter().map(|s| s.service_name().as_str()))
            .collect::<Vec<&str>>();

        assert_eq!(service_names, vec!["service-a"]);

        Ok(())
    }

    #[tokio::test]
    async fn should_create_app_with_base_ingress_route() -> Result<(), AppsServiceError> {
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(Config::default(), infrastructure)?.with_base_route(Some(
            TraefikIngressRoute::with_rule(TraefikRouterRule::host_rule(vec![String::from(
                "example.com",
            )])),
        ));

        let app_name = &AppName::master();
        apps.create_or_update(
            app_name,
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        let services = apps
            .infrastructure
            .as_any()
            .downcast_ref::<Dummy>()
            .unwrap()
            .services();

        assert!(iters_equal_anyorder(
            services
                .iter()
                .flat_map(|s| s.ingress_route().routes().iter())
                .map(|route| route.rule()),
            [
                TraefikRouterRule::from_str(
                    "Host(`example.com`) && PathPrefix(`/master/service-b/`)"
                )
                .unwrap(),
                TraefikRouterRule::from_str(
                    "Host(`example.com`) && PathPrefix(`/master/service-a/`)"
                )
                .unwrap()
            ]
            .iter()
        ));

        Ok(())
    }

    #[tokio::test]
    async fn should_create_app_without_base_ingress_route() -> Result<(), AppsServiceError> {
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(Config::default(), infrastructure)?;

        let app_name = &AppName::master();
        apps.create_or_update(
            app_name,
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        let services = apps
            .infrastructure
            .as_any()
            .downcast_ref::<Dummy>()
            .unwrap()
            .services();

        assert!(iters_equal_anyorder(
            services
                .iter()
                .flat_map(|s| s.ingress_route().routes().iter())
                .map(|route| route.rule()),
            [
                TraefikRouterRule::path_prefix_rule(["/master/service-a"]),
                TraefikRouterRule::path_prefix_rule(["/master/service-b"])
            ]
            .iter()
        ));

        Ok(())
    }

    fn iters_equal_anyorder<T: Eq + Hash>(
        mut i1: impl Iterator<Item = T>,
        i2: impl Iterator<Item = T>,
    ) -> bool {
        let set: HashSet<T> = i2.collect();
        i1.all(|x| set.contains(&x))
    }

    #[tokio::test]
    async fn do_not_create_app_when_exceeding_application_number_limit(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [applications]
            max = 1
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        let result = apps
            .create_or_update(
                &AppName::from_str("other").unwrap(),
                None,
                &vec![sc!("service-a"), sc!("service-b")],
                vec![],
                None,
            )
            .await;

        assert!(matches!(
            result,
            Err(AppsServiceError::AppLimitExceeded { limit: 1 })
        ));

        Ok(())
    }

    #[tokio::test]
    async fn do_update_app_when_exceeding_application_number_limit() -> Result<(), AppsServiceError>
    {
        let config = config_from_str!(
            r#"
            [applications]
            max = 1
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            vec![],
            None,
        )
        .await?;

        let result = apps
            .create_or_update(
                &AppName::master(),
                None,
                &vec![sc!("service-c")],
                vec![],
                None,
            )
            .await;

        assert!(matches!(
            result,
            Ok(app) if app.services().len() == 3
        ));

        Ok(())
    }

    #[tokio::test]
    async fn deploy_companion_with_user_defined_data() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.db1]
            serviceName = 'db1-{{userDefined.name}}'
            type = 'service'
            image = 'postgres:16.1'

            [companions.templating.userDefinedSchema]
            type = "object"
            properties = { name = { type = "string" } }
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::master();
        apps.create_or_update(
            &app_name,
            None,
            &vec![sc!("web-service")],
            vec![],
            Some(serde_json::json!({
                "name": "my-name"
            })),
        )
        .await?;

        let companion_config: Vec<ServiceConfig> = apps
            .infrastructure
            .fetch_app(&app_name)
            .await?
            .map(|app| app.into_services())
            .unwrap()
            .into_iter()
            .filter(|service| service.service_name() == "db1-my-name")
            .map(|service| service.config)
            .collect();

        assert_eq!(companion_config[0].service_name(), "db1-my-name");

        Ok(())
    }

    #[tokio::test]
    async fn clone_companion_with_user_defined_data() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.db1]
            serviceName = 'db1-{{userDefined.name}}'
            type = 'service'
            image = 'postgres:16.1'

            [companions.templating.userDefinedSchema]
            type = "object"
            properties = { name = { type = "string" } }
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::master();
        apps.create_or_update(
            &app_name,
            None,
            &vec![sc!("web-service")],
            vec![],
            Some(serde_json::json!({
                "name": "my-name"
            })),
        )
        .await?;

        let replicated_app_name = AppName::from_str("replica").unwrap();
        apps.create_or_update(&replicated_app_name, None, &[], vec![], None)
            .await?;

        let companion_config: Vec<ServiceConfig> = apps
            .infrastructure
            .fetch_app(&replicated_app_name)
            .await?
            .map(|app| app.into_services())
            .unwrap()
            .into_iter()
            .filter(|service| service.service_name() == "db1-my-name")
            .map(|service| service.config)
            .collect();

        assert_eq!(companion_config[0].service_name(), "db1-my-name");

        Ok(())
    }

    #[tokio::test]
    async fn update_owners() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a")],
            vec![Owner {
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                name: None,
            }],
            None,
        )
        .await?;
        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-b")],
            vec![Owner {
                iss: IssuerUrl::new(String::from("https://github.com")).unwrap(),
                sub: SubjectIdentifier::new(String::from("github-user")),
                name: None,
            }],
            None,
        )
        .await?;

        let mut deployed_apps = apps.fetch_apps().await?;
        assert_eq!(deployed_apps.len(), 1);
        let app = deployed_apps.remove(&AppName::master()).unwrap();

        let (_, owners) = app.into_services_and_owners();
        assert_eq!(
            owners,
            HashSet::from([
                Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: None,
                },
                Owner {
                    sub: SubjectIdentifier::new(String::from("github-user")),
                    iss: IssuerUrl::new(String::from("https://github.com")).unwrap(),
                    name: None,
                }
            ])
        );

        Ok(())
    }

    #[tokio::test]
    async fn merge_owners_with_same_sub_issuer() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-a")],
            vec![Owner {
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                name: None,
            }],
            None,
        )
        .await?;
        apps.create_or_update(
            &AppName::master(),
            None,
            &vec![sc!("service-b")],
            vec![Owner {
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                name: None,
            }],
            None,
        )
        .await?;

        let mut deployed_apps = apps.fetch_apps().await?;
        assert_eq!(deployed_apps.len(), 1);
        let app = deployed_apps.remove(&AppName::master()).unwrap();

        let (_, owners) = app.into_services_and_owners();
        assert_eq!(
            owners,
            HashSet::from([Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: None,
            },])
        );

        Ok(())
    }
}
