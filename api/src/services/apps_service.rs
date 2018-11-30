/*-
 * ========================LICENSE_START=================================
 * PREvant
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
use std::collections::HashMap;
use std::convert::{From, TryFrom};
use std::sync::mpsc;

use models::service::{ContainerType, Service, ServiceConfig, ServiceError};
use multimap::MultiMap;
use services::config_service::Config;
use shiplift::builder::ContainerOptions;
use shiplift::errors::Error as ShipLiftError;
use shiplift::rep::Container;
use shiplift::{ContainerFilter, ContainerListOptions, Docker, ExecContainerOptions, PullOptions};

static APP_NAME_LABEL: &str = "preview.servant.app-name";
static SERVICE_NAME_LABEL: &str = "preview.servant.service-name";
static CONTAINER_TYPE_LABEL: &str = "preview.servant.container-type";

pub struct AppsService {}

impl AppsService {
    pub fn new() -> AppsService {
        AppsService {}
    }

    /// Analyzes running containers and returns a map of `review-app-name` with the
    /// corresponding list of `Service`s.
    pub fn get_apps(&self) -> Result<MultiMap<String, Service>, AppsServiceError> {
        let apps: MultiMap<String, Service> = self.get_services(|_| true)?;
        Ok(apps)
    }

    fn get_services_vec(&self, app_name: &String) -> Result<Vec<Service>, AppsServiceError> {
        match self.get_services(|app| app_name == app)?.get_vec(app_name) {
            None => Ok(vec![]),
            Some(services) => Ok(services.clone()),
        }
    }

    fn get_services<AppFilter>(
        &self,
        app_name_filter: AppFilter,
    ) -> Result<MultiMap<String, Service>, AppsServiceError>
    where
        AppFilter: Fn(&String) -> bool,
    {
        let mut apps: MultiMap<String, Service> = MultiMap::new();

        let docker = Docker::new();
        let containers = docker.containers();

        let f = ContainerFilter::LabelName(String::from(APP_NAME_LABEL));
        for c in containers.list(&ContainerListOptions::builder().filter(vec![f]).build())? {
            let app_name = c.labels.get(APP_NAME_LABEL).unwrap().to_string();

            if !app_name_filter(&app_name) {
                continue;
            }

            match Service::try_from(c) {
                Ok(service) => apps.insert(app_name, service),
                Err(e) => debug!("Container does not provide required data: {:?}", e),
            }
        }

        let master_containers = self.get_app_containers(&"master".to_owned())?;
        let master_services = master_containers
            .iter()
            .map(|c| (c.labels.get(SERVICE_NAME_LABEL).unwrap().to_string(), c))
            .collect::<HashMap<String, &Container>>();

        let app_service_names = apps
            .iter_all()
            .filter(|(app_name, _services)| app_name.as_str() != "master")
            .map(|(app_name, services)| {
                (
                    app_name.clone(),
                    services
                        .iter()
                        .map(|s: &Service| s.get_service_name().clone())
                        .collect::<Vec<String>>(),
                )
            }).collect::<Vec<(String, Vec<String>)>>();

        for app_service in app_service_names {
            let (app_name, service_names) = app_service;

            for linked_container in master_services
                .iter()
                .filter(|s| !service_names.contains(s.0))
                .map(|lc| lc.1)
            {
                let mut linked_service = Service::try_from(linked_container.to_owned().to_owned())?;
                linked_service.set_app_name(&app_name);
                linked_service.set_container_type(ContainerType::Linked);
                apps.insert(app_name.to_owned(), linked_service);
            }
        }

        Ok(apps)
    }

    /// Creates or updates a review app with the given service configurations
    pub fn create_or_update(
        &self,
        app_name: &String,
        service_configs: &Vec<ServiceConfig>,
    ) -> Result<Vec<Service>, AppsServiceError> {
        let mut configs: Vec<ContainerConfiguration> = service_configs
            .iter()
            .map(|c| ContainerConfiguration::from(c.clone()))
            .collect();

        if "master" != app_name {
            configs.extend(self.get_replicated_configs_of_master_app(app_name, service_configs)?);
        }

        let services = self.start_containers(app_name, configs.iter())?;

        self.update_host_entries(app_name)?;

        Ok(services)
    }

    /// Deletes all services for the given `app_name` (review app name)
    pub fn delete_app(&self, app_name: &String) -> Result<Vec<Service>, AppsServiceError> {
        let services = match self.get_services(|a| app_name == a)?.get_vec(app_name) {
            None => return Err(AppsServiceError::AppNotFound(app_name.clone())),
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
            if let Err(e) = rx.recv()? {
                return Err(e);
            }
        }

        Ok(services.to_vec())
    }

    fn delete_container(
        &self,
        app_name: &String,
        service: &Service,
    ) -> Result<(), AppsServiceError> {
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

    fn start_containers<'a, ConfigsIterator>(
        &self,
        app_name: &String,
        configs: ConfigsIterator,
    ) -> Result<Vec<Service>, AppsServiceError>
    where
        ConfigsIterator: Iterator<Item = &'a ContainerConfiguration>,
    {
        let mut count = 0;
        let (tx, rx) = mpsc::channel();

        crossbeam_utils::thread::scope(|scope| {
            for config in configs {
                count += 1;

                let tx_clone = tx.clone();
                scope.spawn(move || {
                    let service = self.start_container(app_name, config);
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

    fn start_container(
        &self,
        app_name: &String,
        config: &ContainerConfiguration,
    ) -> Result<Service, AppsServiceError> {
        let docker = Docker::new();
        let containers = docker.containers();
        let images = docker.images();

        if let None = config.image_id {
            let image = config.get_image()?;
            let tag = config.get_image_tag();

            info!(
                "Pulling {:?} for {:?} of app {:?}",
                image,
                config.config.get_service_name(),
                app_name
            );

            let pull_options = PullOptions::builder()
                .image(image.clone())
                .tag(tag.clone())
                .build();

            for o in images.pull(&pull_options) {
                debug!("{:?}", o);
            }
        }

        let mut image_to_delete = None;
        if let Some(ref container_info) =
            self.get_app_container(app_name, config.get_service_name())
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

        let container_type_name = config.container_type.to_string();
        let image = config.get_image()?;

        info!(
            "Creating new review app container for {:?}: service={:?} with image={:?} ({:?})",
            app_name,
            config.get_service_name(),
            image,
            container_type_name
        );

        let mut options = ContainerOptions::builder(&image);
        if let Some(ref env) = config.config.get_env() {
            options.env(env.iter().map(|e| e.as_str()).collect());
        }

        if let Some(ref volumes) = config.config.get_volumes() {
            options.volumes(volumes.iter().map(|v| v.as_str()).collect());
        }

        let traefik_frontend = format!(
            "ReplacePathRegex: ^/{p1}/{p2}(.*) /$1;PathPrefix:/{p1}/{p2};",
            p1 = app_name,
            p2 = config.get_service_name()
        );
        let mut labels: HashMap<&str, &str> = HashMap::new();
        labels.insert(APP_NAME_LABEL, app_name);
        labels.insert(SERVICE_NAME_LABEL, &config.get_service_name());
        labels.insert(CONTAINER_TYPE_LABEL, &container_type_name);
        labels.insert("traefik.frontend.rule", &traefik_frontend);

        options.labels(&labels);
        options.restart_policy("always", 5);

        match Config::load() {
            Err(err) => warn!("Cannot load config: no memory limit {:?}", err),
            Ok(config) => {
                if let Some(memory_limit) = config.get_container_config().get_memory_limit() {
                    options.memory(memory_limit.clone());
                }
            }
        }

        let container_info = containers.create(&options.build())?;
        debug!("Created container: {:?}", container_info);

        containers.get(&container_info.id).start()?;
        debug!("Started container: {:?}", container_info);

        let mut service =
            Service::try_from(self.get_app_container_by_id(&container_info.id).unwrap())?;
        service.set_container_type(config.container_type.clone());

        if let Some(image) = image_to_delete {
            info!("Clean up image {:?} of app {:?}", image, app_name);
            match images.get(&image).delete() {
                Ok(output) => for o in output {
                    debug!("{:?}", o);
                },
                Err(err) => debug!("Could not clean up image: {:?}", err),
            }
        }

        Ok(service)
    }

    fn get_replicated_configs_of_master_app(
        &self,
        app_name: &String,
        service_configs: &Vec<ServiceConfig>,
    ) -> Result<Vec<ContainerConfiguration>, AppsServiceError> {
        let running_services: Vec<String> = self
            .get_services_vec(app_name)?
            .iter()
            .filter(|s| {
                s.get_container_type() == &ContainerType::Linked
                    || s.get_container_type() == &ContainerType::Instance
            }).map(|s| s.get_service_name().clone())
            .collect();

        let master_containers: Vec<Container> = self
            .get_app_containers(&"master".to_owned())?
            .iter()
            .filter(|c| {
                let service_name = &c.labels.get(SERVICE_NAME_LABEL).unwrap();
                service_configs
                    .iter()
                    .map(|c| c.get_service_name())
                    .find(|s| s == service_name)
                    .map_or_else(|| true, |_| false)
            }).filter(|c| !running_services.contains(&c.labels.get(SERVICE_NAME_LABEL).unwrap()))
            .map(|c| c.clone())
            .collect();

        let mut configs: Vec<ContainerConfiguration> = Vec::new();

        let docker = Docker::new();
        let containers = docker.containers();
        for container_info in master_containers {
            let container = containers.get(&container_info.id);
            let details = container.inspect()?;

            let env: Option<Vec<String>> = match details.config.env {
                None => None,
                Some(env) => Some(
                    env.iter()
                        .filter(|env| !env.starts_with("HIBERNATE_DIALECT"))
                        .filter(|env| !env.starts_with("JDBC_DRIVER"))
                        .filter(|env| !env.starts_with("JDBC_URL"))
                        .map(|env| env.to_owned())
                        .collect(),
                ),
            };

            let service_name = container_info
                .labels
                .get(SERVICE_NAME_LABEL)
                .unwrap()
                .clone();

            configs.push(ContainerConfiguration {
                config: ServiceConfig::new(&service_name, None, env),
                container_type: ContainerType::Replica,
                image_id: Some(details.image.clone()),
            });
        }

        Ok(configs)
    }

    /// Iterates over all created containers and creates entries in `/etc/hosts` to refer to the
    /// other containers of the `app_name`.
    fn update_host_entries(&self, app_name: &String) -> Result<(), AppsServiceError> {
        info!("Linking containers to each other for {:?}", app_name);

        let docker = Docker::new();
        let containers = docker.containers();

        let app_containers = self.get_app_containers(app_name)?;

        for container_info in &app_containers {
            for remote_container_info in app_containers.iter().filter(|c| c.id != container_info.id)
            {
                let details = containers.get(&remote_container_info.id).inspect()?;

                match remote_container_info.labels.get(SERVICE_NAME_LABEL) {
                    None => error!(
                        "Cannot get service-name label of container {:?}",
                        remote_container_info.id
                    ),
                    Some(service_name) => {
                        let cmd = "echo \"".to_owned()
                            + &details.network_settings.ip_address
                            + "\t"
                            + service_name
                            + "\" >> /etc/hosts";
                        let options = ExecContainerOptions::builder()
                            .cmd(vec!["sh", "-c", &cmd])
                            .attach_stdout(true)
                            .attach_stderr(true)
                            .build();

                        containers.get(&container_info.id).exec(&options)?;
                    }
                }
            }
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
            }).filter(|c| match c.labels.get(SERVICE_NAME_LABEL) {
                None => false,
                Some(service) => service == service_name,
            }).map(|c| c.to_owned())
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

    fn get_app_containers(&self, app_name: &String) -> Result<Vec<Container>, AppsServiceError> {
        let f1 = ContainerFilter::Label(APP_NAME_LABEL.to_owned(), app_name.clone());

        let docker = Docker::new();
        let containers = docker.containers();

        let list_options = ContainerListOptions::builder().filter(vec![f1]).build();

        Ok(containers.list(&list_options)?)
    }
}

/// Defines error cases for the `AppService`
#[derive(Debug)]
pub enum AppsServiceError {
    /// Will be used when the service configuration is invalid that has been request by the client
    InvalidServiceModel(ServiceError),
    /// Will be used when the image for a service could not be found.
    ImageNotFound(String),
    /// Will be used when no app with a given name is found
    AppNotFound(String),
    /// Will be used when the service cannot interact correctly with the docker daemon.
    DockerInteraction(String),
    ThreadingError(std::sync::mpsc::RecvError),
}

struct ContainerConfiguration {
    config: ServiceConfig,
    container_type: ContainerType,
    image_id: Option<String>,
}

impl ContainerConfiguration {
    fn get_image(&self) -> Result<String, AppsServiceError> {
        match &self.image_id {
            None => Ok(self.config.get_docker_image()?),
            Some(id) => Ok(id.clone()),
        }
    }

    fn get_image_tag(&self) -> String {
        self.config.get_image_tag()
    }

    fn get_service_name(&self) -> &String {
        self.config.get_service_name()
    }
}

impl From<ServiceConfig> for ContainerConfiguration {
    fn from(service_config: ServiceConfig) -> Self {
        ContainerConfiguration {
            config: service_config,
            container_type: ContainerType::Instance,
            image_id: None,
        }
    }
}

impl std::fmt::Display for ContainerConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "({}, {})",
            self.config.get_service_name(),
            self.container_type
        )
    }
}

impl From<ServiceError> for AppsServiceError {
    fn from(err: ServiceError) -> Self {
        AppsServiceError::InvalidServiceModel(err)
    }
}

impl From<ShipLiftError> for AppsServiceError {
    fn from(err: ShipLiftError) -> Self {
        match &err {
            ShipLiftError::Fault { code, message } => match code.as_u16() {
                404u16 => AppsServiceError::ImageNotFound(message.to_string()),
                _ => AppsServiceError::DockerInteraction(err.to_string()),
            },
            err => AppsServiceError::DockerInteraction(err.to_string()),
        }
    }
}

impl From<std::sync::mpsc::RecvError> for AppsServiceError {
    fn from(err: std::sync::mpsc::RecvError) -> Self {
        AppsServiceError::ThreadingError(err)
    }
}

impl TryFrom<Container> for Service {
    type Error = ServiceError;

    fn try_from(c: Container) -> Result<Service, ServiceError> {
        let service_name = match c.labels.get(SERVICE_NAME_LABEL) {
            Some(name) => name,
            None => return Err(ServiceError::MissingServiceNameLabel),
        };

        let app_name = match c.labels.get(APP_NAME_LABEL) {
            Some(name) => name,
            None => return Err(ServiceError::MissingReviewAppNameLabel),
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
