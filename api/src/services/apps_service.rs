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

use crate::models::request_info::RequestInfo;
use crate::models::service::{ContainerType, Service, ServiceConfig};
use crate::models::web_host_meta::WebHostMeta;
use crate::services::config_service::{Config, ConfigError};
use crate::services::service_templating::{
    apply_templating_for_application_companion, apply_templating_for_service_companion,
};
use crate::services::{
    images_service::{ImagesService, ImagesServiceError},
    infrastructure::Infrastructure,
};
use handlebars::TemplateRenderError;
use http_api_problem::{HttpApiProblem, StatusCode};
use multimap::MultiMap;
use std::collections::HashSet;
use std::convert::From;
use std::sync::Mutex;
use url::Url;

pub struct AppsService {
    config: Config,
    infrastructure: Box<dyn Infrastructure>,
    apps_in_deployment: Mutex<HashSet<String>>,
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
        })
    }

    fn resolve_web_host_meta(
        app_name: &String,
        service_name: &String,
        endpoint_url: &Url,
        request_info: &RequestInfo,
    ) -> Option<WebHostMeta> {
        let url = endpoint_url.join(".well-known/host-meta.json").unwrap();

        let get_request = reqwest::Client::builder()
            .build()
            .unwrap()
            .get(url)
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
                format!("/{}/{}", app_name, service_name),
            )
            .header("Accept", "application/json")
            .header("User-Agent", format!("PREvant/{}", crate_version!()))
            .send();

        match get_request {
            Err(err) => {
                debug!("Cannot acquire host meta: {}", err);
                None
            }
            Ok(mut response) => match response.json::<WebHostMeta>() {
                Err(err) => {
                    error!("Cannot parse host meta: {}", err);
                    Some(WebHostMeta::empty())
                }
                Ok(meta) => Some(meta),
            },
        }
    }

    /// Analyzes running containers and returns a map of `app-name` with the
    /// corresponding list of `Service`s.
    pub fn get_apps(
        &self,
        request_info: &RequestInfo,
    ) -> Result<MultiMap<String, Service>, AppsServiceError> {
        let mut services = self.infrastructure.get_services()?;

        for (app_name, services) in services.iter_all_mut() {
            for service in services {
                service.set_web_host_meta(match service.endpoint_url() {
                    None => None,
                    Some(endpoint_url) => AppsService::resolve_web_host_meta(
                        app_name,
                        service.service_name(),
                        &endpoint_url,
                        request_info,
                    ),
                });
            }
        }

        Ok(services)
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

    /// Creates or updates a app to review with the given service configurations.
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

        let mut configs: Vec<ServiceConfig> = service_configs.clone();

        let replicate_from_app_name = replicate_from.unwrap_or_else(|| String::from("master"));
        if &replicate_from_app_name != app_name {
            for config in self
                .infrastructure
                .get_configs_of_app(&replicate_from_app_name)?
                .iter()
                .filter(|config| {
                    service_configs
                        .iter()
                        .find(|c| c.service_name() == config.service_name())
                        .is_none()
                })
            {
                let mut replicated_config = config.clone();
                replicated_config.set_container_type(ContainerType::Replica);
                configs.push(replicated_config);
            }
        }

        let images_service = ImagesService::new();
        let mappings = images_service.resolve_image_ports(&configs)?;
        for config in configs.iter_mut() {
            if let Some(port) = mappings.get(config) {
                config.set_port(port.clone());
            }
        }

        let mut service_companions = Vec::new();
        for config in &configs {
            let companions = self.config.get_service_companion_configs()?;

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

        configs.extend(service_companions);
        configs.extend(self.get_application_companion_configs(app_name, &configs)?);

        configs.sort_unstable_by(|a, b| {
            let index1 = AppsService::container_type_index(a.container_type());
            let index2 = AppsService::container_type_index(b.container_type());
            index1.cmp(&index2)
        });

        let services = self.infrastructure.start_services(
            app_name,
            &configs,
            &self.config.get_container_config(),
        )?;

        Ok(services)
    }

    fn get_application_companion_configs(
        &self,
        app_name: &String,
        service_configs: &Vec<ServiceConfig>,
    ) -> Result<Vec<ServiceConfig>, AppsServiceError> {
        let mut configs_for_templating = service_configs.clone();

        // TODO: make sure that service companions are included!
        for config in self
            .infrastructure
            .get_configs_of_app(app_name)?
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
        for app_companion_config in self.config.get_application_companion_configs()? {
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
        match self.infrastructure.get_services()?.get_vec(app_name) {
            None => Err(AppsServiceError::AppNotFound {
                app_name: app_name.clone(),
            }),
            Some(_) => Ok(self.infrastructure.stop_services(app_name)?),
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

    use super::*;
    use crate::models::service::Image;
    use crate::services::dummy_infrastructure::DummyInfrastructure;
    use std::str::FromStr;

    #[test]
    fn should_create_app_for_master() {
        let app_name = String::from("master");
        let services = vec![ServiceConfig::new(
            String::from("service-a"),
            Image::from_str(
                "sha256:541b21b43bdd8f1547599d0350713d82c74c9a72c13cfd47e742b377ea638ee2",
            )
            .unwrap(),
        )];

        let config = Config::default();
        let infrastructure = Box::new(DummyInfrastructure::new());
        let apps = AppsService::new(config, infrastructure).unwrap();

        apps.create_or_update(&app_name, None, &services).unwrap();

        let info = RequestInfo::new(Url::parse("http://example.com").unwrap());

        let deployed_apps = apps.get_apps(&info).unwrap();
        assert_eq!(deployed_apps.len(), 1);
        let services = deployed_apps.get_vec("master").unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services.get(0).unwrap().service_name(), "service-a")
    }

}
