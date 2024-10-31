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
mod routes;

pub use crate::apps::AppsService as Apps;
pub use crate::apps::AppsServiceError as AppsError;
use crate::config::{Config, ConfigError};
use crate::deployment::deployment_unit::DeploymentUnitBuilder;
use crate::infrastructure::Infrastructure;
use crate::models::service::{ContainerType, Service, ServiceStatus};
use crate::models::user_defined_parameters::UserDefinedParameters;
use crate::models::{AppName, AppStatusChangeId, LogChunk, ServiceConfig};
use crate::registry::Registry;
use crate::registry::RegistryError;
use chrono::{DateTime, FixedOffset};
use futures::stream::BoxStream;
use futures::StreamExt;
use handlebars::RenderError;
pub use host_meta_cache::new as host_meta_crawling;
pub use host_meta_cache::HostMetaCache;
use multimap::MultiMap;
pub use routes::{apps_routes, delete_app_sync};
use std::collections::{HashMap, HashSet};
use std::convert::From;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub struct AppsService {
    config: Config,
    infrastructure: Box<dyn Infrastructure>,
    app_guards: Mutex<HashMap<AppName, Arc<AppGuard>>>,
}

type GuardedResult = Result<Vec<Service>, AppsServiceError>;

#[derive(Debug, Copy, Clone, PartialEq)]
enum AppGuardKind {
    Deployment,
    Deletion,
}

/// This helper struct ensures that there is only one thread interacting with an application through
/// the infrastructure while multiple threads requests want to interact at the same time.
///
/// With [#40](https://github.com/aixigo/PREvant/issues/40) this should be replaced with an worker/
/// producer approach.
struct AppGuard {
    app_name: AppName,
    kind: AppGuardKind,
    process_mutex: Mutex<(bool, Option<GuardedResult>)>,
    condvar: Condvar,
}

impl AppGuard {
    fn new(app_name: AppName, kind: AppGuardKind) -> Self {
        AppGuard {
            app_name,
            kind,
            process_mutex: Mutex::new((false, None)),
            condvar: Condvar::new(),
        }
    }

    fn is_first(&self) -> bool {
        let mut guard = self.process_mutex.lock().unwrap();
        if guard.0 {
            false
        } else {
            guard.0 = true;
            true
        }
    }

    fn wait_for_result(&self) -> GuardedResult {
        let mut guard = self.process_mutex.lock().unwrap();
        while guard.1.is_none() {
            trace!("waiting for the result of {}", self.app_name);
            guard = self.condvar.wait(guard).unwrap();
        }
        guard
            .1
            .as_ref()
            .cloned()
            .expect("Here it is expected that the deletion result is always present")
    }

    fn notify_with_result(
        &self,
        apps_service: &AppsService,
        result: GuardedResult,
    ) -> GuardedResult {
        let mut guard = self.process_mutex.lock().unwrap();
        guard.1 = Some(result.clone());
        self.condvar.notify_all();

        let mut apps_in_deletion = apps_service.app_guards.lock().unwrap();
        let removed_guard = apps_in_deletion.remove(&self.app_name);
        trace!(
            "Dropped guard for {:?}",
            removed_guard.as_ref().map(|g| &*g.app_name)
        );

        result
    }
}

impl AppsService {
    pub fn new(
        config: Config,
        infrastructure: Box<dyn Infrastructure>,
    ) -> Result<AppsService, AppsServiceError> {
        Ok(AppsService {
            config,
            infrastructure,
            app_guards: Mutex::new(HashMap::new()),
        })
    }

    pub fn infrastructure(&self) -> &dyn Infrastructure {
        self.infrastructure.as_ref()
    }

    /// Analyzes running containers and returns a map of `app-name` with the
    /// corresponding list of `Service`s.
    pub async fn get_apps(&self) -> Result<MultiMap<AppName, Service>, AppsServiceError> {
        Ok(self.infrastructure.get_services().await?)
    }

    fn create_or_get_app_guard(
        &self,
        app_name: AppName,
        kind: AppGuardKind,
    ) -> Result<Arc<AppGuard>, AppsServiceError> {
        let mut apps_in_deletion = self.app_guards.lock().unwrap();
        let guard = &*apps_in_deletion
            .entry(app_name.clone())
            .or_insert_with(|| Arc::new(AppGuard::new(app_name.clone(), kind)));

        if guard.kind != kind {
            match guard.kind {
                AppGuardKind::Deletion => Err(AppsServiceError::AppIsInDeletion { app_name }),
                AppGuardKind::Deployment => Err(AppsServiceError::AppIsInDeployment { app_name }),
            }
        } else {
            Ok(guard.clone())
        }
    }

    async fn configs_to_replicate(
        &self,
        services_to_deploy: &[ServiceConfig],
        app_name: &AppName,
        replicate_from_app_name: &AppName,
    ) -> Result<Vec<ServiceConfig>, AppsServiceError> {
        let running_services = self.infrastructure.get_configs_of_app(app_name).await?;
        let running_service_names = running_services
            .iter()
            .filter(|c| c.container_type() == &ContainerType::Instance)
            .map(|c| c.service_name())
            .collect::<HashSet<&String>>();

        let service_names = services_to_deploy
            .iter()
            .map(|c| c.service_name())
            .collect::<HashSet<&String>>();

        Ok(self
            .infrastructure
            .get_configs_of_app(replicate_from_app_name)
            .await?
            .into_iter()
            .filter(|config| !service_names.contains(config.service_name()))
            .filter(|config| !running_service_names.contains(config.service_name()))
            .map(|config| {
                let mut replicated_config = config;
                replicated_config.set_container_type(ContainerType::Replica);
                replicated_config
            })
            .collect::<Vec<ServiceConfig>>())
    }

    pub async fn wait_for_status_change(
        &self,
        status_id: &AppStatusChangeId,
    ) -> Result<Vec<Service>, AppsServiceError> {
        let mut services = Vec::new();
        while let Some(s) = self
            .infrastructure
            .get_status_change(&status_id.to_string())
            .await?
        {
            services = s;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(services)
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
    pub async fn create_or_update(
        &self,
        app_name: &AppName,
        status_id: &AppStatusChangeId,
        replicate_from: Option<AppName>,
        service_configs: &[ServiceConfig],
        user_defined_parameters: Option<serde_json::Value>,
    ) -> Result<Vec<Service>, AppsServiceError> {
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

        let guard = self.create_or_get_app_guard(app_name.clone(), AppGuardKind::Deployment)?;

        if !guard.is_first() {
            return Err(AppsServiceError::AppIsInDeployment {
                app_name: app_name.clone(),
            });
        }

        guard.notify_with_result(
            self,
            self.create_or_update_impl(
                app_name,
                status_id,
                replicate_from,
                service_configs,
                user_defined_parameters,
            )
            .await,
        )
    }

    async fn create_or_update_impl(
        &self,
        app_name: &AppName,
        status_id: &AppStatusChangeId,
        replicate_from: Option<AppName>,
        service_configs: &[ServiceConfig],
        user_defined_parameters: Option<UserDefinedParameters>,
    ) -> Result<Vec<Service>, AppsServiceError> {
        if let Some(app_limit) = self.config.app_limit() {
            let apps = self.get_apps().await?;

            if apps
                .keys()
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

        let replicate_from_app_name = replicate_from.unwrap_or_else(AppName::master);
        if &replicate_from_app_name != app_name {
            configs.extend(
                self.configs_to_replicate(service_configs, app_name, &replicate_from_app_name)
                    .await?,
            );
        }

        let configs_for_templating = self
            .infrastructure
            .get_configs_of_app(app_name)
            .await?
            .into_iter()
            .filter(|config| config.container_type() == &ContainerType::Instance)
            .filter(|config| {
                !service_configs
                    .iter()
                    .any(|c| c.service_name() == config.service_name())
            })
            .collect::<Vec<_>>();

        let deployment_unit_builder = DeploymentUnitBuilder::init(app_name.clone(), configs)
            .extend_with_config(&self.config)
            .extend_with_templating_only_service_configs(configs_for_templating);

        let images = deployment_unit_builder.images();
        let image_infos = Registry::new(&self.config)
            .resolve_image_infos(&images)
            .await?;

        let base_traefik_ingress_route = self
            .infrastructure
            .base_traefik_ingress_route()
            .await
            .ok()
            .flatten();

        let deployment_unit_builder = deployment_unit_builder
            .extend_with_image_infos(image_infos)
            .apply_templating(
                &base_traefik_ingress_route.as_ref().and_then(|r| r.to_url()),
                user_defined_parameters,
            )?
            .apply_hooks(&self.config)
            .await?;

        let deployment_unit = if let Some(base_traefik_ingress_route) = base_traefik_ingress_route {
            trace!(
                "The base URL for {app_name} is: {:?}",
                base_traefik_ingress_route
                    .to_url()
                    .map(|url| url.to_string())
            );
            deployment_unit_builder
                .apply_base_traefik_ingress_route(base_traefik_ingress_route)
                .build()
        } else {
            deployment_unit_builder.build()
        };

        let services = self
            .infrastructure
            .deploy_services(
                &status_id.to_string(),
                &deployment_unit,
                &self.config.container_config(),
            )
            .await?;

        Ok(services)
    }

    /// Deletes all services for the given `app_name`.
    pub async fn delete_app(
        &self,
        app_name: &AppName,
        status_id: &AppStatusChangeId,
    ) -> Result<Vec<Service>, AppsServiceError> {
        let guard = self.create_or_get_app_guard(app_name.clone(), AppGuardKind::Deletion)?;

        if !guard.is_first() {
            guard.wait_for_result()
        } else {
            guard.notify_with_result(self, self.delete_app_impl(app_name, status_id).await)
        }
    }

    async fn delete_app_impl(
        &self,
        app_name: &AppName,
        status_id: &AppStatusChangeId,
    ) -> Result<Vec<Service>, AppsServiceError> {
        let services = self
            .infrastructure
            .stop_services(&status_id.to_string(), app_name)
            .await?;
        if services.is_empty() {
            Err(AppsServiceError::AppNotFound {
                app_name: app_name.clone(),
            })
        } else {
            Ok(services)
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
#[derive(Debug, Clone, thiserror::Error)]
pub enum AppsServiceError {
    #[error("Cannot find app {app_name}.")]
    AppNotFound { app_name: AppName },
    #[error("Cannot create more than {limit} apps")]
    AppLimitExceeded { limit: usize },
    #[error("The app {app_name} is currently within deployment by another request.")]
    AppIsInDeployment { app_name: AppName },
    #[error("The app {app_name} is currently within deletion in by another request.")]
    AppIsInDeletion { app_name: AppName },
    /// Will be used when the service cannot interact correctly with the infrastructure.
    #[error("Cannot interact with infrastructure: {error}")]
    InfrastructureError { error: Arc<anyhow::Error> },
    /// Will be used if the service configuration cannot be loaded.
    #[error("Invalid configuration: {error}")]
    InvalidServerConfiguration { error: Arc<ConfigError> },
    #[error("Invalid configuration (invalid template): {error}")]
    InvalidTemplateFormat { error: Arc<RenderError> },
    #[error("Unable to resolve information about image: {error}")]
    UnableToResolveImage { error: Arc<RegistryError> },
    #[error("Invalid deployment hook.")]
    InvalidDeploymentHook,
    #[error("Failed to parse traefik rule ({raw_rule}): {err}")]
    FailedToParseTraefikRule { raw_rule: String, err: String },
    #[error("User defined payload does not match to the configured value: {err}")]
    InvalidUserDefinedParameters { err: String },
}

impl From<ConfigError> for AppsServiceError {
    fn from(error: ConfigError) -> Self {
        AppsServiceError::InvalidServerConfiguration {
            error: Arc::new(error),
        }
    }
}

impl From<anyhow::Error> for AppsServiceError {
    fn from(error: anyhow::Error) -> Self {
        AppsServiceError::InfrastructureError {
            error: Arc::new(error),
        }
    }
}

impl From<RenderError> for AppsServiceError {
    fn from(error: RenderError) -> Self {
        AppsServiceError::InvalidTemplateFormat {
            error: Arc::new(error),
        }
    }
}

impl From<RegistryError> for AppsServiceError {
    fn from(error: RegistryError) -> Self {
        AppsServiceError::UnableToResolveImage {
            error: Arc::new(error),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::infrastructure::{Dummy, TraefikIngressRoute, TraefikRouterRule};
    use crate::models::{EnvironmentVariable, ServiceBuilder};
    use crate::sc;
    use chrono::Utc;
    use futures::StreamExt;
    use secstr::SecUtf8;
    use std::hash::Hash;
    use std::io::Write;
    use std::path::PathBuf;
    use std::str::FromStr;
    use tempfile::NamedTempFile;
    use tokio::runtime;

    macro_rules! config_from_str {
        ( $config_str:expr ) => {
            toml::from_str::<Config>($config_str).unwrap()
        };
    }

    macro_rules! assert_contains_service {
        ( $services:expr, $service_name:expr, $container_type:expr ) => {
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
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a")],
            None,
        )
        .await?;

        let deployed_apps = apps.get_apps().await?;
        assert_eq!(deployed_apps.len(), 1);
        let services = deployed_apps.get_vec(&AppName::master()).unwrap();
        assert_eq!(services.len(), 1);
        assert_contains_service!(services, "service-a", ContainerType::Instance);

        Ok(())
    }

    #[tokio::test]
    async fn should_replication_from_master() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            None,
        )
        .await?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            &AppStatusChangeId::new(),
            Some(AppName::master()),
            &vec![sc!("service-b")],
            None,
        )
        .await?;

        let deployed_apps = apps.get_apps().await?;

        let services = deployed_apps
            .get_vec(&AppName::from_str("branch").unwrap())
            .unwrap();
        assert_eq!(services.len(), 2);
        assert_contains_service!(services, "service-b", ContainerType::Instance);
        assert_contains_service!(services, "service-a", ContainerType::Replica);

        Ok(())
    }

    #[tokio::test]
    async fn should_override_replicas_from_master() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::master(),
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            None,
        )
        .await?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            &AppStatusChangeId::new(),
            Some(AppName::master()),
            &vec![sc!("service-b")],
            None,
        )
        .await?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            &AppStatusChangeId::new(),
            Some(AppName::master()),
            &vec![sc!("service-a")],
            None,
        )
        .await?;

        let deployed_apps = apps.get_apps().await?;

        let services = deployed_apps
            .get_vec(&AppName::from_str("branch").unwrap())
            .unwrap();
        assert_eq!(services.len(), 2);
        assert_contains_service!(services, "service-a", ContainerType::Instance);
        assert_contains_service!(services, "service-b", ContainerType::Instance);

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
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("mariadb")],
            None,
        )
        .await?;

        let configs = apps
            .infrastructure
            .get_configs_of_app(&AppName::master())
            .await?;
        assert_eq!(configs.len(), 1);

        let files = configs.get(0).unwrap().files().unwrap();
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
            &AppName::from_str("master-1.x").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("mariadb")],
            None,
        )
        .await?;

        let configs = apps
            .infrastructure
            .get_configs_of_app(&AppName::from_str("master-1.x").unwrap())
            .await?;
        assert_eq!(configs.len(), 1);

        let files = configs.get(0).unwrap().files();
        assert_eq!(files, None);

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
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
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
        apps.create_or_update(&app_name, &AppStatusChangeId::new(), None, &services, None)
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
        apps.create_or_update(
            &app_name,
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a")],
            None,
        )
        .await?;
        let deployed_apps = apps.get_apps().await?;

        let services = deployed_apps.get_vec(&app_name).unwrap();
        assert_eq!(services.len(), 3);
        assert_contains_service!(services, "openid", ContainerType::ApplicationCompanion);
        assert_contains_service!(services, "db", ContainerType::ServiceCompanion);
        assert_contains_service!(services, "service-a", ContainerType::Instance);

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
        apps.create_or_update(&app_name, &AppStatusChangeId::new(), None, &configs, None)
            .await?;
        let deployed_apps = apps.get_apps().await?;

        let services = deployed_apps.get_vec(&app_name).unwrap();
        assert_eq!(services.len(), 2);
        assert_contains_service!(services, "openid", ContainerType::Instance);
        assert_contains_service!(services, "db", ContainerType::Instance);

        let openid_configs: Vec<ServiceConfig> = apps
            .infrastructure
            .get_configs_of_app(&app_name)
            .await?
            .into_iter()
            .filter(|config| config.service_name() == "openid")
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

        apps.create_or_update(&app_name, &AppStatusChangeId::new(), None, &configs, None)
            .await?;

        let deployed_apps = apps.get_apps().await?;

        let services = deployed_apps.get_vec(&app_name).unwrap();
        assert_eq!(services.len(), 1);
        assert_contains_service!(services, "openid", ContainerType::Instance);

        let openid_configs: Vec<ServiceConfig> = apps
            .infrastructure
            .get_configs_of_app(&app_name)
            .await?
            .into_iter()
            .filter(|config| config.service_name() == "openid")
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
            &AppStatusChangeId::new(),
            None,
            &vec![crate::sc!("service-a")],
            None,
        )
        .await?;
        apps.create_or_update(
            &app_name,
            &AppStatusChangeId::new(),
            None,
            &vec![crate::sc!("service-b")],
            None,
        )
        .await?;
        apps.create_or_update(
            &app_name,
            &AppStatusChangeId::new(),
            None,
            &vec![crate::sc!("service-c")],
            None,
        )
        .await?;

        let services = apps.infrastructure.get_services().await?;
        let openid_config = services
            .get_vec(&AppName::master())
            .unwrap()
            .iter()
            .find(|service| service.service_name() == "openid")
            .map(|service| service.config())
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
        apps.create_or_update(
            &app_name,
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a")],
            None,
        )
        .await?;
        let deleted_services = apps
            .delete_app(&app_name, &AppStatusChangeId::new())
            .await?;

        assert_eq!(
            deleted_services,
            vec![ServiceBuilder::new()
                .id("service-a".to_string())
                .app_name("master".to_string())
                .config(crate::sc!("service-a"))
                .started_at(
                    DateTime::parse_from_rfc3339("2019-07-18T07:25:00.000000000Z")
                        .unwrap()
                        .with_timezone(&Utc),
                )
                .build()
                .unwrap()],
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_delete_apps_from_parallel_threads_returning_the_same_result(
    ) -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::with_delay(std::time::Duration::from_millis(500)));
        let apps = Arc::new(AppsService::new(config, infrastructure)?);

        let app_name = AppName::master();
        apps.create_or_update(
            &app_name,
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a")],
            None,
        )
        .await?;

        let apps_clone = apps.clone();
        let handle1 = std::thread::spawn(move || {
            let rt = runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            rt.block_on(apps_clone.delete_app(&app_name, &AppStatusChangeId::new()))
        });
        let app_name = AppName::master();
        let handle2 = std::thread::spawn(move || {
            let rt = runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            rt.block_on(apps.delete_app(&app_name, &AppStatusChangeId::new()))
        });

        assert_eq!(handle1.join().unwrap()?, handle2.join().unwrap()?,);

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
        apps.create_or_update(&app_name, &AppStatusChangeId::new(), None, &configs, None)
            .await?;
        let deployed_apps = apps.get_apps().await?;

        let db_config1: Vec<ServiceConfig> = apps
            .infrastructure
            .get_configs_of_app(&app_name)
            .await?
            .into_iter()
            .filter(|config| config.service_name() == "db1")
            .collect();

        let db_config2: Vec<ServiceConfig> = apps
            .infrastructure
            .get_configs_of_app(&app_name)
            .await?
            .into_iter()
            .filter(|config| config.service_name() == "db2")
            .collect();

        let services = deployed_apps.get_vec(&app_name).unwrap();

        assert_eq!(services.len(), 2);
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
            &app_name,
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            None,
        )
        .await?;

        let deployed_services = apps
            .get_apps()
            .await?
            .into_iter()
            .map(|(_, services)| services.into_iter().map(|s| s.service_name().clone()))
            .flatten()
            .collect::<Vec<_>>();

        assert_eq!(deployed_services, vec![String::from("service-a")]);

        Ok(())
    }

    #[tokio::test]
    async fn should_create_app_with_base_ingress_route() -> Result<(), AppsServiceError> {
        let infrastructure = Box::new(Dummy::with_base_route(TraefikIngressRoute::with_rule(
            TraefikRouterRule::host_rule(vec![String::from("example.com")]),
        )));
        let apps = AppsService::new(Config::default(), infrastructure)?;

        let app_name = &AppName::master();
        apps.create_or_update(
            &app_name,
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
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
            &app_name,
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
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
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            None,
        )
        .await?;

        let result = apps
            .create_or_update(
                &AppName::from_str("other").unwrap(),
                &AppStatusChangeId::new(),
                None,
                &vec![sc!("service-a"), sc!("service-b")],
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
            &AppStatusChangeId::new(),
            None,
            &vec![sc!("service-a"), sc!("service-b")],
            None,
        )
        .await?;

        let result = apps
            .create_or_update(
                &AppName::master(),
                &AppStatusChangeId::new(),
                None,
                &vec![sc!("service-c")],
                None,
            )
            .await;

        assert!(matches!(
            dbg!(result),
            Ok(services) if services.len() == 3
        ));

        Ok(())
    }
}
