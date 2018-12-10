/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 aixigo AG
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
use models::service::{ContainerType, Service, ServiceConfig, ServiceError};
use multimap::MultiMap;
use services::infrastructure::Infrastructure;

use super::super::config_service::ContainerConfig;
use failure::Error;
use models;
use shiplift::builder::ContainerOptions;
use shiplift::errors::Error as ShipLiftError;
use shiplift::rep::Container;
use shiplift::{
    ContainerConnectionOptions, ContainerFilter, ContainerListOptions, Docker,
    NetworkCreateOptions, PullOptions,
};
use std::collections::HashMap;
use std::convert::{From, TryFrom};
use std::sync::mpsc;

static APP_NAME_LABEL: &str = "preview.servant.app-name";
static SERVICE_NAME_LABEL: &str = "preview.servant.service-name";
static CONTAINER_TYPE_LABEL: &str = "preview.servant.container-type";

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
        if let Some(n) = docker
            .networks()
            .list(&Default::default())?
            .iter()
            .find(|n| &n.name == &network_name)
        {
            return Ok(n.id.clone());
        }

        debug!("Creating network for app {}.", app_name);

        let network_create_info = docker
            .networks()
            .create(&NetworkCreateOptions::builder(network_name.as_ref()).build())?;

        debug!(
            "Created network for app {} with id {}",
            app_name, network_create_info.id
        );

        Ok(network_create_info.id)
    }

    fn delete_network(&self, app_name: &String) -> Result<(), ShipLiftError> {
        let network_name = format!("{}-net", app_name);

        let docker = Docker::new();
        for n in docker
            .networks()
            .list(&Default::default())?
            .iter()
            .filter(|n| &n.name == &network_name)
        {
            docker.networks().get(&n.id).delete()?;
        }

        Ok(())
    }

    fn delete_container(&self, app_name: &String, service: &Service) -> Result<(), ShipLiftError> {
        let docker = Docker::new();
        let containers = docker.containers();
        if let Some(c) = self.get_app_container(app_name, service.get_service_name()) {
            let container = containers.get(&c.id);

            container.stop(None)?;
            info!(
                "Stopped service {:?} (container id={:?}) for review app {:?}",
                service.get_service_name(),
                c.id,
                app_name
            );

            container.delete()?;
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

        if !service_config.refers_to_image_id() {
            self.pull_image(app_name, &service_config)?;
        }

        let mut image_to_delete = None;
        if let Some(ref container_info) =
            self.get_app_container(app_name, service_config.get_service_name())
        {
            let container = containers.get(&container_info.id);
            let container_image_id = container.inspect().unwrap().image.clone();

            info!(
                "Removing container {:?} of review app {:?}",
                container_info, app_name
            );
            container.stop(Some(core::time::Duration::from_secs(10)))?;
            container.delete()?;

            image_to_delete = Some(container_image_id.clone());
        }

        let container_type_name = service_config.get_container_type().to_string();
        let image = service_config.get_docker_image();

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

        if let Some(ref volumes) = service_config.get_volumes() {
            options.volumes(volumes.iter().map(|v| v.as_str()).collect());
        }

        let traefik_frontend = format!(
            "ReplacePathRegex: ^/{p1}/{p2}(.*) /$1;PathPrefix:/{p1}/{p2};",
            p1 = app_name,
            p2 = service_config.get_service_name()
        );
        let mut labels: HashMap<&str, &str> = HashMap::new();
        labels.insert(APP_NAME_LABEL, app_name);
        labels.insert(SERVICE_NAME_LABEL, &service_config.get_service_name());
        labels.insert(CONTAINER_TYPE_LABEL, &container_type_name);
        labels.insert("traefik.frontend.rule", &traefik_frontend);

        options.labels(&labels);
        options.restart_policy("always", 5);

        if let Some(memory_limit) = container_config.get_memory_limit() {
            options.memory(memory_limit.clone());
        }

        let container_info = containers.create(&options.build())?;
        debug!("Created container: {:?}", container_info);

        containers.get(&container_info.id).start()?;
        debug!("Started container: {:?}", container_info);

        docker.networks().get(network_id).connect(
            &ContainerConnectionOptions::builder(&container_info.id)
                .aliases(vec![service_config.get_service_name().as_str()])
                .build(),
        )?;
        debug!(
            "Connected container {:?} to {:?}",
            container_info.id, network_id
        );

        let mut service =
            Service::try_from(&self.get_app_container_by_id(&container_info.id).unwrap())?;
        service.set_container_type(service_config.get_container_type().clone());

        if let Some(image) = image_to_delete {
            info!("Clean up image {:?} of app {:?}", image, app_name);
            match images.get(&image).delete() {
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

    fn pull_image(&self, app_name: &String, config: &ServiceConfig) -> Result<(), ShipLiftError> {
        let image = config.get_docker_image();

        info!(
            "Pulling {:?} for {:?} of app {:?}",
            image,
            config.get_service_name(),
            app_name
        );

        let pull_options = PullOptions::builder().image(image).build();

        let docker = Docker::new();
        let images = docker.images();
        for o in images.pull(&pull_options) {
            debug!("{:?}", o);
        }

        Ok(())
    }

    fn get_app_container(&self, app_name: &String, service_name: &String) -> Option<Container> {
        let docker = Docker::new();
        let containers = docker.containers();

        let list_options = ContainerListOptions::builder().build();

        containers
            .list(&list_options)
            .unwrap()
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
            .next()
    }

    fn get_app_container_by_id(&self, container_id: &String) -> Option<Container> {
        let docker = Docker::new();
        let containers = docker.containers();

        let list_options = ContainerListOptions::builder().build();

        containers
            .list(&list_options)
            .unwrap()
            .iter()
            .filter(|c| container_id == &c.id)
            .map(|c| c.to_owned())
            .next()
    }

    fn get_app_containers(&self, app_name: &String) -> Result<Vec<Container>, ShipLiftError> {
        let f1 = ContainerFilter::Label(APP_NAME_LABEL.to_owned(), app_name.clone());

        let docker = Docker::new();
        let containers = docker.containers();

        let list_options = ContainerListOptions::builder().filter(vec![f1]).build();

        Ok(containers.list(&list_options)?)
    }
}

impl Infrastructure for DockerInfrastructure {
    fn get_services(&self) -> Result<MultiMap<String, Service>, Error> {
        let mut apps: MultiMap<String, Service> = MultiMap::new();

        let docker = Docker::new();
        let containers = docker.containers();

        let f = ContainerFilter::LabelName(String::from(APP_NAME_LABEL));
        for c in containers.list(&ContainerListOptions::builder().filter(vec![f]).build())? {
            let app_name = c.labels.get(APP_NAME_LABEL).unwrap().to_string();

            match Service::try_from(&c) {
                Ok(service) => apps.insert(app_name, service),
                Err(e) => debug!("Container does not provide required data: {:?}", e),
            }
        }

        Ok(apps)
    }

    fn start_services(
        &self,
        app_name: &String,
        configs: &Vec<ServiceConfig>,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
        let network_id = self.create_or_get_network_id(app_name)?;

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

        let (tx, rx) = mpsc::channel();

        crossbeam_utils::thread::scope(|scope| {
            for service in &services {
                let tx_clone = tx.clone();
                scope.spawn(move || {
                    let result = self.delete_container(app_name, service);
                    tx_clone.send(result).unwrap();
                });
            }
        });

        for _ in &services {
            rx.recv()??;
        }

        self.delete_network(app_name)?;

        Ok(services)
    }

    fn get_configs_of_app(&self, app_name: &String) -> Result<Vec<ServiceConfig>, Error> {
        let mut configs = Vec::new();

        let docker = Docker::new();
        let containers = docker.containers();
        for container in self.get_app_containers(app_name)? {
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

            let details = containers.get(&container.id).inspect()?;
            let env: Option<Vec<String>> = match details.config.env {
                None => None,
                Some(env) => Some(env.clone()),
            };

            // TODO: clone volume data...

            println!("{}", &container.image);

            let (repo, user, registry, tag) =
                models::service::parse_image_string(&container.image)?;
            let mut service_config = ServiceConfig::new(service.get_service_name(), &repo, env);

            service_config.set_image_user(&user);
            service_config.set_registry(&registry);
            service_config.set_image_tag(&tag);

            configs.push(service_config);
        }

        Ok(configs)
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
                })
            }
        };

        let app_name = match c.labels.get(APP_NAME_LABEL) {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingAppNameLabel {
                    container_id: c.id.clone(),
                })
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
            _err => DockerInfrastructureError::UnexpectedError {
                internal_message: String::from(""),
            },
        }
    }
}
