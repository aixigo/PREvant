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

use super::super::config_service::ContainerConfig;
use crate::models::service::{ContainerType, Image, Service, ServiceConfig, ServiceError};
use crate::services::infrastructure::Infrastructure;
use failure::Error;
use futures::future::join_all;
use futures::{Future, Stream};
use multimap::MultiMap;
use regex::Regex;
use shiplift::builder::ContainerOptions;
use shiplift::errors::Error as ShipLiftError;
use shiplift::rep::{Container, ContainerCreateInfo};
use shiplift::{
    ContainerConnectionOptions, ContainerFilter, ContainerListOptions, Docker,
    NetworkCreateOptions, PullOptions,
};
use std::collections::HashMap;
use std::convert::{From, TryFrom};
use std::path::Path;
use std::str::FromStr;
use std::sync::mpsc;
use tokio::runtime::Runtime;

static APP_NAME_LABEL: &str = "com.aixigo.preview.servant.app-name";
static SERVICE_NAME_LABEL: &str = "com.aixigo.preview.servant.service-name";
static CONTAINER_TYPE_LABEL: &str = "com.aixigo.preview.servant.container-type";

pub struct DockerInfrastructure {}

#[derive(Debug, Fail)]
pub enum DockerInfrastructureError {
    #[fail(display = "Could not find image: {}", internal_message)]
    ImageNotFound { internal_message: String },
    #[fail(
        display = "The container {} does not provide a label for service name.",
        container_id
    )]
    MissingServiceNameLabel { container_id: String },
    #[fail(
        display = "The container {} does not provide a label for app name.",
        container_id
    )]
    MissingAppNameLabel { container_id: String },
    #[fail(display = "Unexpected docker interaction error: {}", internal_message)]
    UnexpectedError { internal_message: String },
    #[fail(display = "Unknown service type label: {}", unknown_label)]
    UnknownServiceType { unknown_label: String },
}

impl DockerInfrastructure {
    pub fn new() -> DockerInfrastructure {
        DockerInfrastructure {}
    }

    fn create_or_get_network_id(&self, app_name: &String) -> Result<String, ShipLiftError> {
        let network_name = format!("{}-net", app_name);

        let docker = Docker::new();
        let mut runtime = Runtime::new()?;
        let network_id = runtime
            .block_on(docker.networks().list(&Default::default()))?
            .iter()
            .find(|n| &n.name == &network_name)
            .map(|n| n.id.clone());

        if let Some(n) = network_id {
            return Ok(n.clone());
        }

        debug!("Creating network for app {}.", app_name);

        let network_create_info = runtime.block_on(
            docker
                .networks()
                .create(&NetworkCreateOptions::builder(network_name.as_ref()).build()),
        )?;

        debug!(
            "Created network for app {} with id {}",
            app_name, network_create_info.id
        );

        Ok(network_create_info.id)
    }

    fn connect_traefik(&self, network_id: &String) -> Result<(), ShipLiftError> {
        let docker = Docker::new();
        let mut runtime = Runtime::new()?;

        let traefik_container_id = runtime.block_on(
            docker
                .containers()
                .list(&ContainerListOptions::builder().build())
                .map(move |containers| {
                    containers
                        .into_iter()
                        .find(|c| c.image.contains("traefik"))
                        .map(|c| c.id)
                }),
        )?;

        if let Some(id) = traefik_container_id {
            if let Err(e) = runtime.block_on(
                docker
                    .networks()
                    .get(network_id)
                    .connect(&ContainerConnectionOptions::builder(&id).build()),
            ) {
                debug!("Cannot traefik: {}", e);
            }
        }

        Ok(())
    }

    fn disconnect_traefik(&self, network_id: &String) -> Result<(), ShipLiftError> {
        let docker = Docker::new();
        let mut runtime = Runtime::new()?;

        let traefik_container_id = runtime.block_on(
            docker
                .containers()
                .list(&ContainerListOptions::builder().build())
                .map(move |containers| {
                    containers
                        .into_iter()
                        .find(|c| c.image.contains("traefik"))
                        .map(|c| c.id)
                }),
        )?;

        if let Some(id) = traefik_container_id {
            runtime.block_on(
                docker
                    .networks()
                    .get(network_id)
                    .disconnect(&ContainerConnectionOptions::builder(&id).build()),
            )?;
        }

        Ok(())
    }

    fn delete_network(&self, app_name: &String) -> Result<(), ShipLiftError> {
        let network_name = format!("{}-net", app_name);

        let docker = Docker::new();
        let mut runtime = Runtime::new()?;
        for n in runtime
            .block_on(docker.networks().list(&Default::default()))?
            .iter()
            .filter(|n| &n.name == &network_name)
        {
            self.disconnect_traefik(&n.id)?;
            runtime.block_on(docker.networks().get(&n.id).delete())?;
        }

        Ok(())
    }

    fn start_container(
        &self,
        app_name: &String,
        network_id: &String,
        service_config: &ServiceConfig,
        container_config: &ContainerConfig,
    ) -> Result<Service, Error> {
        let docker = Docker::new();
        let containers = docker.containers();
        let images = docker.images();
        let mut runtime = Runtime::new()?;

        if let Image::Named { .. } = service_config.get_image() {
            self.pull_image(&mut runtime, app_name, &service_config)?;
        }

        let mut image_to_delete = None;
        if let Some(ref container_info) =
            self.get_app_container(app_name, service_config.get_service_name())?
        {
            let container = containers.get(&container_info.id);
            let container_image_id = runtime.block_on(container.inspect())?.image.clone();

            info!(
                "Removing container {:?} of review app {:?}",
                container_info, app_name
            );
            runtime.block_on(container.stop(Some(core::time::Duration::from_secs(10))))?;
            runtime.block_on(container.delete())?;

            image_to_delete = Some(container_image_id.clone());
        }

        let container_type_name = service_config.get_container_type().to_string();
        let image = service_config.get_image().to_string();

        info!(
            "Creating new review app container for {:?}: service={:?} with image={:?} ({:?})",
            app_name,
            service_config.get_service_name(),
            image,
            container_type_name
        );

        let mut options = ContainerOptions::builder(&image);
        if let Some(ref env) = service_config.get_env() {
            options.env(env.iter().map(|e| e.as_str()).collect());
        }

        let traefik_frontend = format!(
            "ReplacePathRegex: ^/{app_name}/{service_name}/(.*) /$1;PathPrefix:/{app_name}/{service_name}/;",
            app_name = app_name,
            service_name = service_config.get_service_name()
        );
        let mut labels: HashMap<&str, &str> = HashMap::new();
        labels.insert("traefik.frontend.rule", &traefik_frontend);

        if let Some(config_labels) = service_config.get_labels() {
            for (k, v) in config_labels {
                labels.insert(k, v);
            }
        }

        labels.insert(APP_NAME_LABEL, app_name);
        labels.insert(SERVICE_NAME_LABEL, &service_config.get_service_name());
        labels.insert(CONTAINER_TYPE_LABEL, &container_type_name);

        options.labels(&labels);
        options.restart_policy("always", 5);

        if let Some(memory_limit) = container_config.get_memory_limit() {
            options.memory(memory_limit.clone());
        }

        let container_info = runtime.block_on(containers.create(&options.build()))?;
        debug!("Created container: {:?}", container_info);

        self.copy_volume_data(&container_info, service_config)?;

        runtime.block_on(containers.get(&container_info.id).start())?;
        debug!("Started container: {:?}", container_info);

        runtime.block_on(
            docker.networks().get(network_id).connect(
                &ContainerConnectionOptions::builder(&container_info.id)
                    .aliases(vec![service_config.get_service_name().as_str()])
                    .build(),
            ),
        )?;
        debug!(
            "Connected container {:?} to {:?}",
            container_info.id, network_id
        );

        let mut service =
            Service::try_from(&self.get_app_container_by_id(&container_info.id)?.unwrap())?;
        service.set_container_type(service_config.get_container_type().clone());

        if let Some(image) = image_to_delete {
            info!("Clean up image {:?} of app {:?}", image, app_name);
            match runtime.block_on(images.get(&image).delete()) {
                Ok(output) => {
                    for o in output {
                        debug!("{:?}", o);
                    }
                }
                Err(err) => debug!("Could not clean up image: {:?}", err),
            }
        }

        Ok(service)
    }

    fn copy_volume_data(
        &self,
        container_info: &ContainerCreateInfo,
        service_config: &ServiceConfig,
    ) -> Result<(), ShipLiftError> {
        let volumes = match service_config.get_volumes() {
            None => return Ok(()),
            Some(volumes) => volumes,
        };

        debug!("Copy data to container: {:?} (service = {})", container_info, service_config.get_service_name());

        let docker = Docker::new();
        let containers = docker.containers();
        let mut runtime = Runtime::new()?;

        for (path, data) in volumes {
            runtime.block_on(
                containers
                    .get(&container_info.id)
                    .copy_file_into(Path::new(path), &data.as_bytes()),
            )?;
        }

        Ok(())
    }

    fn pull_image(
        &self,
        runtime: &mut Runtime,
        app_name: &String,
        config: &ServiceConfig,
    ) -> Result<(), ShipLiftError> {
        let image = config.get_image().to_string();

        info!(
            "Pulling {:?} for {:?} of app {:?}",
            image,
            config.get_service_name(),
            app_name
        );

        let pull_options = PullOptions::builder().image(image).build();

        let docker = Docker::new();
        let images = docker.images();
        runtime.block_on(images.pull(&pull_options).for_each(|output| {
            debug!("{:?}", output);
            Ok(())
        }))?;

        Ok(())
    }

    fn get_app_container(
        &self,
        app_name: &String,
        service_name: &String,
    ) -> Result<Option<Container>, ShipLiftError> {
        let docker = Docker::new();
        let containers = docker.containers();
        let mut runtime = Runtime::new()?;

        let list_options = ContainerListOptions::builder().build();

        Ok(runtime
            .block_on(containers.list(&list_options))?
            .iter()
            .filter(|c| match c.labels.get(APP_NAME_LABEL) {
                None => false,
                Some(app) => app == app_name,
            })
            .filter(|c| match c.labels.get(SERVICE_NAME_LABEL) {
                None => false,
                Some(service) => service == service_name,
            })
            .map(|c| c.to_owned())
            .next())
    }

    fn get_app_container_by_id(
        &self,
        container_id: &String,
    ) -> Result<Option<Container>, ShipLiftError> {
        let docker = Docker::new();
        let containers = docker.containers();
        let mut runtime = Runtime::new()?;

        let list_options = ContainerListOptions::builder().build();

        Ok(runtime
            .block_on(containers.list(&list_options))?
            .iter()
            .filter(|c| container_id == &c.id)
            .map(|c| c.to_owned())
            .next())
    }
}

impl Infrastructure for DockerInfrastructure {
    fn get_services(&self) -> Result<MultiMap<String, Service>, Error> {
        let docker = Docker::new();
        let containers = docker.containers();

        let f = ContainerFilter::LabelName(String::from(APP_NAME_LABEL));

        let future = containers
            .list(&ContainerListOptions::builder().filter(vec![f]).build())
            .map(|containers| {
                let mut apps: MultiMap<String, Service> = MultiMap::new();

                for c in containers {
                    let app_name = c.labels.get(APP_NAME_LABEL).unwrap().to_string();

                    match Service::try_from(&c) {
                        Ok(service) => apps.insert(app_name, service),
                        Err(e) => debug!("Container does not provide required data: {:?}", e),
                    }
                }

                apps
            });

        let mut runtime = Runtime::new()?;
        Ok(runtime.block_on(future)?)
    }

    fn start_services(
        &self,
        app_name: &String,
        configs: &Vec<ServiceConfig>,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
        let network_id = self.create_or_get_network_id(app_name)?;

        self.connect_traefik(&network_id)?;

        let mut count = 0;
        let (tx, rx) = mpsc::channel();

        crossbeam_utils::thread::scope(|scope| {
            for service_config in configs {
                count += 1;

                let network_id_clone = network_id.clone();
                let tx_clone = tx.clone();
                scope.spawn(move || {
                    let service = self.start_container(
                        app_name,
                        &network_id_clone,
                        &service_config,
                        container_config,
                    );
                    tx_clone.send(service).unwrap();
                });
            }
        });

        let mut services: Vec<Service> = Vec::new();
        for _ in 0..count {
            services.push(rx.recv()??);
        }

        Ok(services)
    }

    /// Deletes all services for the given `app_name`.
    fn stop_services(&self, app_name: &String) -> Result<Vec<Service>, Error> {
        let services = match self.get_services()?.get_vec(app_name) {
            None => return Ok(vec![]),
            Some(services) => services.clone(),
        };

        let docker = Docker::new();
        let containers = docker.containers();

        let f1 = ContainerFilter::Label(APP_NAME_LABEL.to_owned(), app_name.clone());
        let list_options = ContainerListOptions::builder().filter(vec![f1]).build();

        let future = containers
            .list(&list_options)
            .map(|containers| containers.iter().map(|c| c.id.clone()).collect());

        let mut runtime = Runtime::new()?;
        let container_ids: Vec<String> = runtime.block_on(future)?;

        let mut container_details_futures = Vec::new();
        for id in container_ids {
            let id_clone = id.clone();
            let future = containers
                .get(&id)
                .stop(None)
                .map(move |_| {
                    let docker = Docker::new();
                    let container = docker.containers().get(&id_clone);
                    container.inspect()
                })
                .and_then(|container_details| container_details);

            container_details_futures.push(future);
        }

        self.delete_network(app_name)?;

        Ok(services)
    }

    fn get_configs_of_app(&self, app_name: &String) -> Result<Vec<ServiceConfig>, Error> {
        let docker = Docker::new();
        let containers = docker.containers();

        let f1 = ContainerFilter::Label(APP_NAME_LABEL.to_owned(), app_name.clone());
        let list_options = ContainerListOptions::builder().filter(vec![f1]).build();

        let mut runtime = Runtime::new()?;
        let mut future_configs = Vec::new();
        for container in runtime.block_on(containers.list(&list_options))? {
            let service = match Service::try_from(&container) {
                Err(e) => {
                    warn!(
                        "Container {} does not provide required information: {}",
                        container.id, e
                    );
                    continue;
                }
                Ok(service) => service,
            };

            match service.get_container_type() {
                ContainerType::ApplicationCompanion | ContainerType::ServiceCompanion => continue,
                _ => {}
            };

            let future_details =
                containers
                    .get(&container.id)
                    .inspect()
                    .map(move |container_details| {
                        let env: Option<Vec<String>> = match container_details.config.env {
                            None => None,
                            Some(env) => Some(env.clone()),
                        };

                        // TODO: clone volume data...

                        let image = Image::from_str(&container_details.image).unwrap();
                        let mut service_config =
                            ServiceConfig::new(service.get_service_name().clone(), &image);
                        service_config.set_env(env);

                        if let Some(ports) = container_details.network_settings.ports {
                            let ports_regex = Regex::new(r#"^(?P<port>\d+).*"#).unwrap();

                            let port = ports
                                .keys()
                                .filter_map(|port| ports_regex.captures(port))
                                .map(|captures| {
                                    String::from(captures.name("port").unwrap().as_str())
                                })
                                .filter_map(|port| port.parse::<u16>().ok())
                                .min()
                                .unwrap_or(80);

                            service_config.set_port(port);
                        }

                        service_config
                    });

            future_configs.push(future_details);
        }

        Ok(runtime.block_on(join_all(future_configs))?)
    }
}

impl TryFrom<&Container> for Service {
    type Error = DockerInfrastructureError;

    fn try_from(c: &Container) -> Result<Service, DockerInfrastructureError> {
        let service_name = match c.labels.get(SERVICE_NAME_LABEL) {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingServiceNameLabel {
                    container_id: c.id.clone(),
                });
            }
        };

        let app_name = match c.labels.get(APP_NAME_LABEL) {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingAppNameLabel {
                    container_id: c.id.clone(),
                });
            }
        };

        let container_type = match c.labels.get(CONTAINER_TYPE_LABEL) {
            None => ContainerType::Instance,
            Some(lb) => lb.parse::<ContainerType>()?,
        };

        Ok(Service::new(
            app_name.clone(),
            service_name.clone(),
            c.id.clone(),
            container_type,
        ))
    }
}

impl From<ShipLiftError> for DockerInfrastructureError {
    fn from(err: ShipLiftError) -> Self {
        match &err {
            ShipLiftError::Fault { code, message } => match code.as_u16() {
                404u16 => DockerInfrastructureError::ImageNotFound {
                    internal_message: message.clone(),
                },
                _ => DockerInfrastructureError::UnexpectedError {
                    internal_message: err.to_string(),
                },
            },
            err => DockerInfrastructureError::UnexpectedError {
                internal_message: err.to_string(),
            },
        }
    }
}

impl From<ServiceError> for DockerInfrastructureError {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::InvalidServiceType { label } => {
                DockerInfrastructureError::UnknownServiceType {
                    unknown_label: label,
                }
            }
            err => DockerInfrastructureError::UnexpectedError {
                internal_message: err.to_string(),
            },
        }
    }
}
