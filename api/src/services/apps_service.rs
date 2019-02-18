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

use crate::models::service::{ContainerType, Service, ServiceConfig, ServiceError};
use crate::services::config_service::{Config, ConfigError};
use crate::services::docker::docker_infrastructure::DockerInfrastructure;
use crate::services::service_templating::{
    apply_templating_for_application_companion, apply_templating_for_service_companion,
};
use crate::services::{
    images_service::{ImagesService, ImagesServiceError},
    infrastructure::Infrastructure,
};
use handlebars::TemplateRenderError;
use multimap::MultiMap;
use std::convert::From;

pub struct AppsService<'a> {
    config: &'a Config,
    infrastructure: Box<dyn Infrastructure>,
}

impl<'a> AppsService<'a> {
    pub fn new(config: &'a Config) -> Result<AppsService, AppsServiceError> {
        Ok(AppsService {
            config,
            infrastructure: Box::new(DockerInfrastructure::new()),
        })
    }

    /// Analyzes running containers and returns a map of `app-name` with the
    /// corresponding list of `Service`s.
    pub fn get_apps(&self) -> Result<MultiMap<String, Service>, AppsServiceError> {
        Ok(self.infrastructure.get_services()?)
    }

    /// Creates or updates a app to review with the given service configurations.
    ///
    /// The list of given services will be extended with:
    /// - the replications from the running template application (e.g. master)
    /// - the application companions (see README)
    /// - the service companions (see README)
    pub fn create_or_update(
        &self,
        app_name: &String,
        service_configs: &Vec<ServiceConfig>,
    ) -> Result<Vec<Service>, AppsServiceError> {
        let mut configs: Vec<ServiceConfig> = service_configs.clone();

        if "master" != app_name {
            for config in self
                .infrastructure
                .get_configs_of_app(&String::from("master"))?
                .iter()
                .filter(|config| {
                    match service_configs
                        .iter()
                        .find(|c| c.get_service_name() == config.get_service_name())
                    {
                        None => true,
                        Some(_) => false,
                    }
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

        for app_companion_config in self.config.get_application_companion_configs()? {
            let applied_template_config = apply_templating_for_application_companion(
                &app_companion_config,
                app_name,
                &configs,
            )?;
            configs.push(applied_template_config);
        }

        configs.sort_unstable_by(|a, b| {
            let index1 = AppsService::container_type_index(a.get_container_type());
            let index2 = AppsService::container_type_index(b.get_container_type());
            index1.cmp(&index2)
        });

        let services = self.infrastructure.start_services(
            app_name,
            &configs,
            &self.config.get_container_config(),
        )?;

        Ok(services)
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
            None => Err(AppsServiceError::AppNotFound(app_name.clone())),
            Some(_) => Ok(self.infrastructure.stop_services(app_name)?),
        }
    }
}

/// Defines error cases for the `AppService`
#[derive(Debug)]
pub enum AppsServiceError {
    /// Will be used when the service configuration is invalid that has been request by the client
    InvalidServiceModel(ServiceError),
    /// Will be used when no app with a given name is found
    AppNotFound(String),
    /// Will be used when the service cannot interact correctly with the infrastructure.
    InfrastructureError(failure::Error),
    /// Will be used if the service configuration cannot be loaded.
    InvalidServerConfiguration(ConfigError),
    InvalidTemplateFormat(TemplateRenderError),
    UnableToResolveImage(ImagesServiceError),
}

impl From<ConfigError> for AppsServiceError {
    fn from(err: ConfigError) -> Self {
        AppsServiceError::InvalidServerConfiguration(err)
    }
}

impl From<failure::Error> for AppsServiceError {
    fn from(error: failure::Error) -> Self {
        AppsServiceError::InfrastructureError(error)
    }
}

impl From<TemplateRenderError> for AppsServiceError {
    fn from(error: TemplateRenderError) -> Self {
        AppsServiceError::InvalidTemplateFormat(error)
    }
}

impl From<ImagesServiceError> for AppsServiceError {
    fn from(error: ImagesServiceError) -> Self {
        AppsServiceError::UnableToResolveImage(error)
    }
}
