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

use crate::config::{Config, ConfigError};
use crate::infrastructure::Infrastructure;
use crate::models::request_info::RequestInfo;
use crate::models::service::{ContainerType, Service, ServiceStatus};
use crate::models::web_host_meta::WebHostMeta;
use crate::models::{AppName, LogChunk, ServiceBuilder, ServiceConfig};
use crate::services::images_service::{ImagesService, ImagesServiceError};
use crate::services::service_templating::{
    apply_templating_for_application_companion, apply_templating_for_service_companion,
};
use cached::{Cached, SizedCache};
use chrono::{DateTime, FixedOffset, Utc};
use handlebars::TemplateRenderError;
use http_api_problem::{HttpApiProblem, StatusCode};
use multimap::MultiMap;
use std::collections::HashSet;
use std::convert::From;
use std::sync::Mutex;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use yansi::Paint;

pub struct AppsService {
    config: Config,
    infrastructure: Box<dyn Infrastructure>,
    apps_in_deployment: Mutex<HashSet<String>>,
    web_host_meta_cache: Mutex<SizedCache<String, WebHostMeta>>,
}

struct DeploymentGuard<'a, 'b> {
    apps_service: &'a AppsService,
    app_name: &'b String,
}

impl<'a, 'b> Drop for DeploymentGuard<'a, 'b> {
    fn drop(&mut self) {
        let mut apps_in_deployment = self.apps_service.apps_in_deployment.lock().unwrap();
        apps_in_deployment.remove(self.app_name);
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
            apps_in_deployment: Mutex::new(HashSet::new()),
            web_host_meta_cache: Mutex::new(SizedCache::with_size(500)),
        })
    }

    /// Analyzes running containers and returns a map of `app-name` with the
    /// corresponding list of `Service`s.
    pub fn get_apps(
        &self,
        request_info: &RequestInfo,
    ) -> Result<MultiMap<String, Service>, AppsServiceError> {
        let mut runtime = Runtime::new().expect("Should create runtime");

        let services = runtime
            .block_on(self.infrastructure.get_services())?
            .iter_all()
            .flat_map(|(app_name, services)| {
                services
                    .iter()
                    // avoid cloning when https://github.com/havarnov/multimap/issues/24 has been implemented
                    .map(move |service| (app_name.clone(), service.clone()))
            })
            .collect::<Vec<_>>();

        let (services_with_host_meta, services_without_host_meta): (Vec<_>, Vec<_>) = {
            let mut cache = self.web_host_meta_cache.lock().unwrap();
            services
                .into_iter()
                .partition(|(_app_name, service)| cache.cache_get(service.id()).is_some())
        };

        let resolved_host_meta_infos =
            runtime.block_on(self.resolve_host_meta(request_info, services_without_host_meta));

        let mut apps = MultiMap::new();
        let mut cache = self.web_host_meta_cache.lock().unwrap();
        for (app_name, service, web_host_meta) in resolved_host_meta_infos {
            if web_host_meta.is_valid() {
                cache.cache_set(service.id().clone(), web_host_meta.clone());
            }

            apps.insert(
                app_name.clone(),
                ServiceBuilder::from(service)
                    .web_host_meta(web_host_meta)
                    .base_url(request_info.get_base_url().clone())
                    .build()
                    .unwrap(),
            )
        }

        apps.extend(
            services_with_host_meta
                .into_iter()
                .map(|(app_name, service)| {
                    let host_meta = cache.cache_get(service.id()).unwrap().clone();
                    (
                        app_name,
                        ServiceBuilder::from(service)
                            .web_host_meta(host_meta)
                            .base_url(request_info.get_base_url().clone())
                            .build()
                            .unwrap(),
                    )
                }),
        );

        Ok(apps)
    }

    async fn resolve_host_meta(
        &self,
        request_info: &RequestInfo,
        services_without_host_meta: Vec<(String, Service)>,
    ) -> Vec<(String, Service, WebHostMeta)> {
        let number_of_services = services_without_host_meta.len();
        if number_of_services == 0 {
            return Vec::with_capacity(0);
        }

        trace!("Resolve web host meta for {} services.", number_of_services);

        let (tx, mut rx) = mpsc::channel(number_of_services);

        for (app_name, service) in services_without_host_meta {
            let mut tx = tx.clone();
            let request_info = request_info.clone();
            tokio::spawn(async move {
                let r = AppsService::resolve_web_host_meta(app_name, service, request_info).await;
                if let Err(err) = tx.send(r).await {
                    error!("Cannot send host meta result: {}", err);
                }
            });
        }

        let mut resolved_host_meta_infos = Vec::with_capacity(number_of_services);
        for _c in 0..number_of_services {
            let resolved_host_meta = rx.recv().await.unwrap();
            resolved_host_meta_infos.push(resolved_host_meta);
        }

        resolved_host_meta_infos
    }

    async fn resolve_web_host_meta(
        app_name: String,
        service: Service,
        request_info: RequestInfo,
    ) -> (String, Service, WebHostMeta) {
        let url = match service.endpoint_url() {
            None => return (app_name, service, WebHostMeta::empty()),
            Some(endpoint_url) => endpoint_url.join(".well-known/host-meta.json").unwrap(),
        };

        let get_request = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_millis(750))
            .user_agent(format!("PREvant/{}", crate_version!()))
            .build()
            .unwrap()
            .get(&url.to_string())
            .header(
                "Forwarded",
                format!(
                    "host={};proto={}",
                    request_info.host(),
                    request_info.scheme()
                ),
            )
            .header(
                "X-Forwarded-Prefix",
                format!("/{}/{}", service.app_name(), service.service_name()),
            )
            .header("Accept", "application/json")
            .send()
            .await;

        let meta = match get_request {
            Ok(response) => match response.json::<WebHostMeta>().await {
                Ok(meta) => meta,
                Err(err) => {
                    error!(
                        "Cannot parse host meta for service {} of {}: {}",
                        Paint::magenta(service.service_name()),
                        Paint::magenta(service.app_name()),
                        err
                    );
                    WebHostMeta::empty()
                }
            },
            Err(err) => {
                debug!(
                    "Cannot acquire host meta for service {} of {}: {}",
                    Paint::magenta(service.service_name()),
                    Paint::magenta(service.app_name()),
                    err
                );

                let duration = Utc::now().signed_duration_since(service.started_at().clone());
                if duration >= chrono::Duration::minutes(5) {
                    trace!(
                        "Service {} is running for {}, therefore, it will be assumed that host-meta.json is not available.",
                        Paint::magenta(service.service_name()), duration
                    );
                    WebHostMeta::empty()
                } else {
                    WebHostMeta::invalid()
                }
            }
        };
        (app_name, service, meta)
    }

    fn create_deployment_guard<'a, 'b>(
        &'a self,
        app_name: &'b String,
    ) -> Option<DeploymentGuard<'a, 'b>> {
        let mut apps_in_deployment = self.apps_in_deployment.lock().unwrap();
        match apps_in_deployment.insert(app_name.clone()) {
            true => Some(DeploymentGuard {
                apps_service: self,
                app_name,
            }),
            false => None,
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
                let mut replicated_config = config.clone();
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
        app_name: &String,
        replicate_from: Option<String>,
        service_configs: &Vec<ServiceConfig>,
    ) -> Result<Vec<Service>, AppsServiceError> {
        let _guard = match self.create_deployment_guard(app_name) {
            None => {
                return Err(AppsServiceError::AppIsInDeployment {
                    app_name: app_name.clone(),
                })
            }
            Some(guard) => guard,
        };

        let mut runtime = Runtime::new().expect("Should create runtime");
        let mut configs: Vec<ServiceConfig> = service_configs.clone();

        let replicate_from_app_name = replicate_from.unwrap_or_else(|| String::from("master"));
        if &replicate_from_app_name != app_name {
            configs.extend(runtime.block_on(self.configs_to_replicate(
                service_configs,
                app_name,
                &replicate_from_app_name,
            ))?);
        }

        for config in configs.iter_mut() {
            self.config.add_secrets_to(config, app_name);
        }

        let mut service_companions = Vec::new();
        for config in &configs {
            let companions = self.config.service_companion_configs(app_name)?;

            service_companions.extend(
                companions
                    .iter()
                    .map(|companion_config| {
                        apply_templating_for_service_companion(companion_config, app_name, config)
                    })
                    .filter_map(|r| r.ok())
                    .collect::<Vec<ServiceConfig>>(),
            );
        }

        AppsService::merge_companions_by_service_name(service_companions, &mut configs);
        AppsService::merge_companions_by_service_name(
            runtime.block_on(self.get_application_companion_configs(app_name, &configs))?,
            &mut configs,
        );

        let images_service = ImagesService::new();
        let mappings = runtime.block_on(images_service.resolve_image_ports(&configs))?;
        for config in configs.iter_mut() {
            if let Some(port) = mappings.get(config) {
                config.set_port(port.clone());
            }
        }

        configs.sort_unstable_by(|a, b| {
            let index1 = AppsService::container_type_index(a.container_type());
            let index2 = AppsService::container_type_index(b.container_type());
            index1.cmp(&index2)
        });

        let services = runtime.block_on(self.infrastructure.deploy_services(
            app_name,
            &configs,
            &self.config.container_config(),
        ))?;

        Ok(services)
    }

    /// Adds all companions to service_configs that are not yet present. Also updates
    /// service configs if there is a corresponding companion config.
    fn merge_companions_by_service_name(
        companions: Vec<ServiceConfig>,
        service_configs: &mut Vec<ServiceConfig>,
    ) {
        for service in service_configs.iter_mut() {
            let matching_companion = companions
                .iter()
                .find(|companion| companion.service_name() == service.service_name());

            if let Some(matching_companion) = matching_companion {
                service.merge_with(matching_companion);
            }
        }

        service_configs.extend(AppsService::filter_companions_by_service_name(
            companions,
            service_configs,
        ));
    }

    fn filter_companions_by_service_name(
        companions: Vec<ServiceConfig>,
        service_configs: &Vec<ServiceConfig>,
    ) -> Vec<ServiceConfig> {
        companions
            .into_iter()
            .filter(|companion| {
                service_configs
                    .iter()
                    .find(|c| companion.service_name() == c.service_name())
                    .is_none()
            })
            .collect()
    }

    async fn get_application_companion_configs(
        &self,
        app_name: &String,
        service_configs: &Vec<ServiceConfig>,
    ) -> Result<Vec<ServiceConfig>, AppsServiceError> {
        let mut configs_for_templating = service_configs.clone();

        // TODO: make sure that service companions are included!
        for config in self
            .infrastructure
            .get_configs_of_app(app_name)
            .await?
            .into_iter()
            .filter(|config| {
                service_configs
                    .iter()
                    .find(|c| c.service_name() == config.service_name())
                    .is_none()
            })
        {
            configs_for_templating.push(config);
        }

        let mut companion_configs = Vec::new();
        for app_companion_config in self.config.application_companion_configs(app_name)? {
            let c = apply_templating_for_application_companion(
                &app_companion_config,
                app_name,
                &configs_for_templating,
            )?;

            companion_configs.push(c);
        }

        Ok(companion_configs)
    }

    fn container_type_index(container_type: &ContainerType) -> i32 {
        match container_type {
            ContainerType::ApplicationCompanion => 0,
            ContainerType::ServiceCompanion => 1,
            ContainerType::Instance | ContainerType::Replica => 2,
        }
    }

    /// Deletes all services for the given `app_name`.
    pub fn delete_app(&self, app_name: &String) -> Result<Vec<Service>, AppsServiceError> {
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
        match runtime.block_on(
            self.infrastructure
                .change_status(app_name, service_name, status),
        )? {
            Some(service) => {
                let mut cache = self.web_host_meta_cache.lock().unwrap();
                (*cache).cache_remove(service.id());
                Ok(Some(service))
            }
            None => Ok(None),
        }
    }
}

/// Defines error cases for the `AppService`
#[derive(Debug, Fail)]
pub enum AppsServiceError {
    /// Will be used when no app with a given name is found
    #[fail(display = "Cannot find app {}.", app_name)]
    AppNotFound { app_name: String },
    #[fail(
        display = "The app {} is currently beeing deployed in by another request.",
        app_name
    )]
    AppIsInDeployment { app_name: String },
    /// Will be used when the service cannot interact correctly with the infrastructure.
    #[fail(display = "Cannot interact with infrastructure: {}", error)]
    InfrastructureError { error: failure::Error },
    /// Will be used if the service configuration cannot be loaded.
    #[fail(display = "Invalid configuration: {}", error)]
    InvalidServerConfiguration { error: ConfigError },
    #[fail(display = "Invalid configuration (invalid template): {}", error)]
    InvalidTemplateFormat { error: TemplateRenderError },
    #[fail(display = "Unable to resolve information about image: {}", error)]
    UnableToResolveImage { error: ImagesServiceError },
}

impl From<AppsServiceError> for HttpApiProblem {
    fn from(error: AppsServiceError) -> Self {
        let status = match error {
            AppsServiceError::AppNotFound { app_name: _ } => StatusCode::NOT_FOUND,
            AppsServiceError::AppIsInDeployment { app_name: _ } => StatusCode::CONFLICT,
            AppsServiceError::InfrastructureError { error: _ }
            | AppsServiceError::InvalidServerConfiguration { error: _ }
            | AppsServiceError::InvalidTemplateFormat { error: _ }
            | AppsServiceError::UnableToResolveImage { error: _ } => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };

        HttpApiProblem::with_title_and_type_from_status(status).set_detail(format!("{}", error))
    }
}

impl From<ConfigError> for AppsServiceError {
    fn from(error: ConfigError) -> Self {
        AppsServiceError::InvalidServerConfiguration { error }
    }
}

impl From<failure::Error> for AppsServiceError {
    fn from(error: failure::Error) -> Self {
        AppsServiceError::InfrastructureError { error }
    }
}

impl From<TemplateRenderError> for AppsServiceError {
    fn from(error: TemplateRenderError) -> Self {
        AppsServiceError::InvalidTemplateFormat { error }
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
    use crate::models::{EnvironmentVariable, Image};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use std::str::FromStr;
    use url::Url;

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
            &String::from("master"),
            None,
            &service_configs!("service-a"),
        )?;

        let info = RequestInfo::new(Url::parse("http://example.com").unwrap());

        let deployed_apps = apps.get_apps(&info)?;
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
            &String::from("master"),
            None,
            &service_configs!("service-a", "service-b"),
        )?;

        apps.create_or_update(
            &String::from("branch"),
            Some(String::from("master")),
            &service_configs!("service-b"),
        )?;

        let info = RequestInfo::new(Url::parse("http://example.com").unwrap());
        let deployed_apps = apps.get_apps(&info)?;

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
            &String::from("master"),
            None,
            &service_configs!("service-a", "service-b"),
        )?;

        apps.create_or_update(
            &String::from("branch"),
            Some(String::from("master")),
            &service_configs!("service-b"),
        )?;

        apps.create_or_update(
            &String::from("branch"),
            Some(String::from("master")),
            &service_configs!("service-a"),
        )?;

        let info = RequestInfo::new(Url::parse("http://example.com").unwrap());
        let deployed_apps = apps.get_apps(&info)?;

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

        apps.create_or_update(&String::from("master"), None, &service_configs!("mariadb"))?;

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
            &String::from("master-1.x"),
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

        let info = RequestInfo::new(Url::parse("http://example.com").unwrap());
        let deployed_apps = apps.get_apps(&info)?;

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

        let info = RequestInfo::new(Url::parse("http://example.com").unwrap());
        let deployed_apps = apps.get_apps(&info)?;

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

        let info = RequestInfo::new(Url::parse("http://example.com").unwrap());
        let deployed_apps = apps.get_apps(&info)?;

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
}
