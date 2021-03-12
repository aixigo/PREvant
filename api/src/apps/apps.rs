/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
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

use crate::apps::DeploymentUnit;
use crate::config::{Config, ConfigError};
use crate::infrastructure::Infrastructure;
use crate::models::service::{ContainerType, Service, ServiceStatus};
use crate::models::{AppName, LogChunk, ServiceConfig};
use crate::services::images_service::{ImagesService, ImagesServiceError};
use chrono::{DateTime, FixedOffset};
use handlebars::TemplateRenderError;
use multimap::MultiMap;
use std::collections::{HashMap, HashSet};
use std::convert::{From, TryInto};
use std::str::FromStr;
use std::sync::{Arc, Condvar, Mutex};
use tokio::runtime::Runtime;

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
        if (*guard).0 {
            false
        } else {
            (*guard).0 = true;
            true
        }
    }

    fn wait_for_result(&self) -> GuardedResult {
        let mut guard = self.process_mutex.lock().unwrap();
        while (*guard).1.is_none() {
            trace!("waiting for the result of {}", self.app_name);
            guard = self.condvar.wait(guard).unwrap();
        }
        (*guard)
            .1
            .as_ref()
            .cloned()
            .expect("Here it is expected that the deletion result is always present")
    }

    #[must_use]
    fn notify_with_result(
        &self,
        apps_service: &AppsService,
        result: GuardedResult,
    ) -> GuardedResult {
        let mut guard = self.process_mutex.lock().unwrap();
        (*guard).1 = Some(result.clone());
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

    /// Analyzes running containers and returns a map of `app-name` with the
    /// corresponding list of `Service`s.
    pub fn get_apps(&self) -> Result<MultiMap<String, Service>, AppsServiceError> {
        let mut runtime = Runtime::new().expect("Should create runtime");

        Ok(runtime.block_on(self.infrastructure.get_services())?)
    }

    #[must_use]
    fn create_or_get_app_guard(
        &self,
        app_name: AppName,
        kind: AppGuardKind,
    ) -> Result<Arc<AppGuard>, AppsServiceError> {
        let mut apps_in_deletion = self.app_guards.lock().unwrap();
        let guard = &*apps_in_deletion
            .entry(app_name.clone())
            .or_insert_with(|| Arc::new(AppGuard::new(app_name.clone(), kind)));

        if &guard.kind != &kind {
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
        services_to_deploy: &Vec<ServiceConfig>,
        app_name: &String,
        replicate_from_app_name: &String,
    ) -> Result<Vec<ServiceConfig>, AppsServiceError> {
        let running_services = self.infrastructure.get_configs_of_app(&app_name).await?;
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
            .get_configs_of_app(&replicate_from_app_name)
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

    /// Creates or updates an app to review with the given service configurations.
    ///
    /// The list of given services will be extended with:
    /// - the replications from the running template application (e.g. master)
    /// - the application companions (see README)
    /// - the service companions (see README)
    ///
    /// # Arguments
    /// * `replicate_from` - The application name that is used as a template.
    pub fn create_or_update(
        &self,
        app_name: &AppName,
        replicate_from: Option<AppName>,
        service_configs: &Vec<ServiceConfig>,
    ) -> Result<Vec<Service>, AppsServiceError> {
        let guard = self.create_or_get_app_guard(app_name.clone(), AppGuardKind::Deployment)?;

        if !guard.is_first() {
            return Err(AppsServiceError::AppIsInDeployment {
                app_name: app_name.clone(),
            });
        }

        guard.notify_with_result(
            self,
            self.create_or_update_impl(app_name, replicate_from, service_configs),
        )
    }

    fn create_or_update_impl(
        &self,
        app_name: &AppName,
        replicate_from: Option<AppName>,
        service_configs: &Vec<ServiceConfig>,
    ) -> Result<Vec<Service>, AppsServiceError> {
        let mut runtime = Runtime::new().expect("Should create runtime");
        let mut configs: Vec<ServiceConfig> = service_configs.clone();

        let replicate_from_app_name =
            replicate_from.unwrap_or_else(|| AppName::from_str("master").unwrap());
        if &replicate_from_app_name != app_name {
            configs.extend(runtime.block_on(self.configs_to_replicate(
                service_configs,
                app_name,
                &replicate_from_app_name,
            ))?);
        }

        let mut deployment_unit = DeploymentUnit::new(app_name.clone(), configs);
        deployment_unit.extend_with_config(&self.config);

        let configs_for_templating = runtime
            .block_on(self.infrastructure.get_configs_of_app(app_name))?
            .into_iter()
            .filter(|config| config.container_type() == &ContainerType::Instance)
            .filter(|config| {
                service_configs
                    .iter()
                    .find(|c| c.service_name() == config.service_name())
                    .is_none()
            })
            .collect::<Vec<_>>();
        deployment_unit.extend_with_templating_only_service_configs(configs_for_templating);

        let images = deployment_unit.images();
        let port_mappings = runtime.block_on(ImagesService::new().resolve_image_ports(&images))?;
        deployment_unit.assign_port_mappings(&port_mappings);

        // TODO: return or input parameter?
        let deployment_id = uuid::Uuid::new_v4();

        let configs: Vec<_> = deployment_unit.try_into()?;
        let services = runtime.block_on(self.infrastructure.deploy_services(
            &deployment_id,
            app_name,
            &configs,
            &self.config.container_config(),
        ))?;

        Ok(services)
    }

    /// Deletes all services for the given `app_name`.
    pub fn delete_app(&self, app_name: &AppName) -> Result<Vec<Service>, AppsServiceError> {
        let guard = self.create_or_get_app_guard(app_name.clone(), AppGuardKind::Deletion)?;

        if !guard.is_first() {
            guard.wait_for_result()
        } else {
            guard.notify_with_result(self, self.delete_app_impl(app_name))
        }
    }

    fn delete_app_impl(&self, app_name: &AppName) -> Result<Vec<Service>, AppsServiceError> {
        let mut runtime = Runtime::new().expect("Should create runtime");

        let services = runtime.block_on(self.infrastructure.stop_services(app_name))?;
        if services.is_empty() {
            Err(AppsServiceError::AppNotFound {
                app_name: app_name.clone(),
            })
        } else {
            Ok(services)
        }
    }

    pub fn get_logs(
        &self,
        app_name: &AppName,
        service_name: &String,
        since: &Option<DateTime<FixedOffset>>,
        limit: usize,
    ) -> Result<Option<LogChunk>, AppsServiceError> {
        let mut runtime = Runtime::new().expect("Should create runtime");
        match runtime.block_on(self.infrastructure.get_logs(
            app_name,
            service_name,
            since,
            limit,
        ))? {
            None => Ok(None),
            Some(ref logs) if logs.is_empty() => Ok(None),
            Some(logs) => Ok(Some(LogChunk::from(logs))),
        }
    }

    pub fn change_status(
        &self,
        app_name: &String,
        service_name: &String,
        status: ServiceStatus,
    ) -> Result<Option<Service>, AppsServiceError> {
        let mut runtime = Runtime::new().expect("Should create runtime");
        Ok(runtime.block_on(
            self.infrastructure
                .change_status(app_name, service_name, status),
        )?)
    }
}

/// Defines error cases for the `AppService`
#[derive(Debug, Clone, Fail)]
pub enum AppsServiceError {
    /// Will be used when no app with a given name is found
    #[fail(display = "Cannot find app {}.", app_name)]
    AppNotFound { app_name: AppName },
    #[fail(
        display = "The app {} is currently within deployment by another request.",
        app_name
    )]
    AppIsInDeployment { app_name: AppName },
    #[fail(
        display = "The app {} is currently within deletion in by another request.",
        app_name
    )]
    AppIsInDeletion { app_name: AppName },
    /// Will be used when the service cannot interact correctly with the infrastructure.
    #[fail(display = "Cannot interact with infrastructure: {}", error)]
    InfrastructureError { error: Arc<failure::Error> },
    /// Will be used if the service configuration cannot be loaded.
    #[fail(display = "Invalid configuration: {}", error)]
    InvalidServerConfiguration { error: Arc<ConfigError> },
    #[fail(display = "Invalid configuration (invalid template): {}", error)]
    InvalidTemplateFormat { error: Arc<TemplateRenderError> },
    #[fail(display = "Unable to resolve information about image: {}", error)]
    UnableToResolveImage { error: ImagesServiceError },
}

impl From<ConfigError> for AppsServiceError {
    fn from(error: ConfigError) -> Self {
        AppsServiceError::InvalidServerConfiguration {
            error: Arc::new(error),
        }
    }
}

impl From<failure::Error> for AppsServiceError {
    fn from(error: failure::Error) -> Self {
        AppsServiceError::InfrastructureError {
            error: Arc::new(error),
        }
    }
}

impl From<TemplateRenderError> for AppsServiceError {
    fn from(error: TemplateRenderError) -> Self {
        AppsServiceError::InvalidTemplateFormat {
            error: Arc::new(error),
        }
    }
}

impl From<ImagesServiceError> for AppsServiceError {
    fn from(error: ImagesServiceError) -> Self {
        AppsServiceError::UnableToResolveImage { error }
    }
}

#[cfg(test)]
mod tests {

    use super::super::super::sc;
    use super::*;
    use crate::infrastructure::Dummy;
    use crate::models::{EnvironmentVariable, Image, ServiceBuilder};
    use chrono::Utc;
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use std::str::FromStr;

    macro_rules! config_from_str {
        ( $config_str:expr ) => {
            toml::from_str::<Config>($config_str).unwrap()
        };
    }

    macro_rules! service_configs {
        ( $( $x:expr ),* ) => {
            {
                let mut hasher = Sha256::new();
                let mut temp_vec: Vec<ServiceConfig> = Vec::new();
                $(
                    hasher.input($x);
                    let img_hash = &format!("sha256:{:x}", hasher.result_reset());

                    temp_vec.push(ServiceConfig::new(
                        String::from($x),
                        Image::from_str(&img_hash).unwrap()
                    ));
                )*
                temp_vec
            }
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
                format!(
                    "services should contain {:?} with type {:?}",
                    $service_name, $container_type
                )
            );
        };
    }

    #[test]
    fn should_create_app_for_master() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            None,
            &service_configs!("service-a"),
        )?;

        let deployed_apps = apps.get_apps()?;
        assert_eq!(deployed_apps.len(), 1);
        let services = deployed_apps.get_vec("master").unwrap();
        assert_eq!(services.len(), 1);
        assert_contains_service!(services, "service-a", ContainerType::Instance);

        Ok(())
    }

    #[test]
    fn should_replication_from_master() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            None,
            &service_configs!("service-a", "service-b"),
        )?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            Some(AppName::from_str("master").unwrap()),
            &service_configs!("service-b"),
        )?;

        let deployed_apps = apps.get_apps()?;

        let services = deployed_apps.get_vec("branch").unwrap();
        assert_eq!(services.len(), 2);
        assert_contains_service!(services, "service-b", ContainerType::Instance);
        assert_contains_service!(services, "service-a", ContainerType::Replica);

        Ok(())
    }

    #[test]
    fn should_override_replicas_from_master() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            None,
            &service_configs!("service-a", "service-b"),
        )?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            Some(AppName::from_str("master").unwrap()),
            &service_configs!("service-b"),
        )?;

        apps.create_or_update(
            &AppName::from_str("branch").unwrap(),
            Some(AppName::from_str("master").unwrap()),
            &service_configs!("service-a"),
        )?;

        let deployed_apps = apps.get_apps()?;

        let services = deployed_apps.get_vec("branch").unwrap();
        assert_eq!(services.len(), 2);
        assert_contains_service!(services, "service-a", ContainerType::Instance);
        assert_contains_service!(services, "service-b", ContainerType::Instance);

        Ok(())
    }

    #[test]
    fn should_create_app_for_master_with_secrets() -> Result<(), AppsServiceError> {
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
            &AppName::from_str("master").unwrap(),
            None,
            &service_configs!("mariadb"),
        )?;

        let mut runtime = Runtime::new().expect("Should create runtime");
        let configs = runtime.block_on(
            apps.infrastructure
                .get_configs_of_app(&String::from("master")),
        )?;
        assert_eq!(configs.len(), 1);

        let volumes = configs.get(0).unwrap().volumes().unwrap();
        assert_eq!(
            volumes.get(&PathBuf::from("/run/secrets/user")).unwrap(),
            "Hello"
        );

        Ok(())
    }

    #[test]
    fn should_create_app_for_master_without_secrets_because_of_none_matching_app_selector(
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
            None,
            &service_configs!("mariadb"),
        )?;

        let mut runtime = Runtime::new().expect("Should create runtime");
        let configs = runtime.block_on(
            apps.infrastructure
                .get_configs_of_app(&String::from("master-1.x")),
        )?;
        assert_eq!(configs.len(), 1);

        let volumes = configs.get(0).unwrap().volumes();
        assert_eq!(volumes, None);

        Ok(())
    }

    #[test]
    fn should_collect_log_chunk_from_infrastructure() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::from_str("master").unwrap();

        apps.create_or_update(&app_name, None, &service_configs!("service-a", "service-b"))?;

        let log_chunk = apps
            .get_logs(&app_name, &String::from("service-a"), &None, 100)
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

    #[test]
    fn should_deploy_companions() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'

            [companions.db]
            serviceName = 'db'
            type = 'service'
            image = 'private.example.com/library/db:latest'
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::from_str("master").unwrap();
        apps.create_or_update(&app_name, None, &service_configs!("service-a"))?;
        let deployed_apps = apps.get_apps()?;

        let services = deployed_apps.get_vec("master").unwrap();
        assert_eq!(services.len(), 3);
        assert_contains_service!(services, "openid", ContainerType::ApplicationCompanion);
        assert_contains_service!(services, "db", ContainerType::ServiceCompanion);
        assert_contains_service!(services, "service-a", ContainerType::Instance);

        Ok(())
    }

    #[test]
    fn should_filter_companions_if_services_to_deploy_contain_same_service_name(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'

            [companions.db]
            serviceName = 'db'
            type = 'service'
            image = 'private.example.com/library/db:latest'
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::from_str("master").unwrap();
        let configs = service_configs!("openid", "db");
        apps.create_or_update(&app_name, None, &configs)?;
        let deployed_apps = apps.get_apps()?;

        let services = deployed_apps.get_vec("master").unwrap();
        assert_eq!(services.len(), 2);
        assert_contains_service!(services, "openid", ContainerType::Instance);
        assert_contains_service!(services, "db", ContainerType::Instance);

        let mut runtime = Runtime::new().expect("Should create runtime");
        let openid_configs: Vec<ServiceConfig> = runtime
            .block_on(
                apps.infrastructure
                    .get_configs_of_app(&String::from("master")),
            )?
            .into_iter()
            .filter(|config| config.service_name() == "openid")
            .collect();
        assert_eq!(openid_configs.len(), 1);
        assert_eq!(openid_configs[0].image(), configs[0].image());

        Ok(())
    }

    #[test]
    fn should_merge_with_companion_config_if_services_to_deploy_contain_same_service_name(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ "VAR_1=abcd", "VAR_2=1234" ]

            [companions.openid.labels]
            'traefik.frontend.rule' = 'PathPrefix:/example.com/openid/;'
            'traefik.frontend.priority' = '20000'
        "#
        );
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::from_str("master").unwrap();

        let configs = vec![sc!(
            "openid",
            labels = (),
            env = ("VAR_1" => "efg"),
            volumes = ()
        )];

        apps.create_or_update(&app_name, None, &configs)?;

        let deployed_apps = apps.get_apps()?;

        let services = deployed_apps.get_vec("master").unwrap();
        assert_eq!(services.len(), 1);
        assert_contains_service!(services, "openid", ContainerType::Instance);

        let mut runtime = Runtime::new().expect("Should create runtime");
        let openid_configs: Vec<ServiceConfig> = runtime
            .block_on(
                apps.infrastructure
                    .get_configs_of_app(&String::from("master")),
            )?
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

    #[test]
    fn should_include_running_instance_in_templating() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ """SERVICES={{~#each services~}}{{name}},{{~/each~}}""" ]
        "#
        );

        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;
        let app_name = AppName::from_str("master").unwrap();

        apps.create_or_update(&app_name, None, &vec![sc!("service-a")])?;
        apps.create_or_update(&app_name, None, &vec![sc!("service-b")])?;
        apps.create_or_update(&app_name, None, &vec![sc!("service-c")])?;

        let mut runtime = Runtime::new().expect("Should create runtime");
        let services = runtime.block_on(apps.infrastructure.get_services())?;
        let openid_config = services
            .get_vec("master")
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

    #[test]
    fn should_delete_apps() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let app_name = AppName::from_str("master").unwrap();
        apps.create_or_update(&app_name, None, &service_configs!("service-a"))?;
        let deleted_services = apps.delete_app(&app_name)?;

        assert_eq!(
            deleted_services,
            vec![ServiceBuilder::new()
                .id("service-a".to_string())
                .app_name("master".to_string())
                .config(sc!("service-a"))
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

    #[test]
    fn should_delete_apps_from_parallel_threads_returning_the_same_result(
    ) -> Result<(), AppsServiceError> {
        let config = Config::default();
        let infrastructure = Box::new(Dummy::with_delay(std::time::Duration::from_millis(500)));
        let apps = Arc::new(AppsService::new(config, infrastructure)?);

        let app_name = AppName::from_str("master").unwrap();
        apps.create_or_update(&app_name, None, &service_configs!("service-a"))?;

        let apps_clone = apps.clone();
        let handle1 = std::thread::spawn(move || apps_clone.delete_app(&app_name));
        let app_name = AppName::from_str("master").unwrap();
        let handle2 = std::thread::spawn(move || apps.delete_app(&app_name));

        assert_eq!(handle1.join().unwrap()?, handle2.join().unwrap()?,);

        Ok(())
    }
}
