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

use crate::config::ContainerConfig;
use crate::infrastructure::{
    Infrastructure, APP_NAME_LABEL, CONTAINER_TYPE_LABEL, REPLICATED_ENV_LABEL, SERVICE_NAME_LABEL,
};
use crate::models::service::{ContainerType, Service, ServiceError, ServiceStatus};
use crate::models::{Environment, Image, ServiceBuilder, ServiceBuilderError, ServiceConfig};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use failure::Error;
use futures::future::join_all;
use futures::StreamExt;
use multimap::MultiMap;
use regex::Regex;
use shiplift::builder::ContainerOptions;
use shiplift::errors::Error as ShipLiftError;
use shiplift::rep::{Container, ContainerCreateInfo, ContainerDetails};
use shiplift::tty::TtyChunk;
use shiplift::{
    ContainerConnectionOptions, ContainerFilter, ContainerListOptions, Docker, LogsOptions,
    NetworkCreateOptions, PullOptions,
};
use std::collections::HashMap;
use std::convert::{From, TryFrom};
use std::net::{AddrParseError, IpAddr};
use std::str::FromStr;
use tokio::sync::mpsc;

static CONTAINER_PORT_LABEL: &str = "traefik.port";

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
    #[fail(display = "Unexpected container address: {}", internal_message)]
    InvalidContainerAddress { internal_message: String },
}

impl DockerInfrastructure {
    pub fn new() -> DockerInfrastructure {
        DockerInfrastructure {}
    }

    async fn create_or_get_network_id(&self, app_name: &String) -> Result<String, ShipLiftError> {
        trace!("Resolve network id for {}", app_name);

        let network_name = format!("{}-net", app_name);

        let docker = Docker::new();
        let network_id = docker
            .networks()
            .list(&Default::default())
            .await?
            .iter()
            .find(|n| n.name == network_name)
            .map(|n| n.id.clone());

        if let Some(n) = network_id {
            return Ok(n);
        }

        debug!("Creating network for app {}.", app_name);

        let network_create_info = docker
            .networks()
            .create(&NetworkCreateOptions::builder(network_name.as_ref()).build())
            .await?;

        debug!(
            "Created network for app {} with id {}",
            app_name, network_create_info.id
        );

        Ok(network_create_info.id)
    }

    async fn connect_traefik(&self, network_id: &String) -> Result<(), ShipLiftError> {
        let docker = Docker::new();

        let containers = docker
            .containers()
            .list(&ContainerListOptions::builder().build())
            .await?;
        let traefik_container_id = containers
            .into_iter()
            .find(|c| c.image.contains("traefik"))
            .map(|c| c.id);

        if let Some(id) = traefik_container_id {
            if let Err(e) = docker
                .networks()
                .get(network_id)
                .connect(&ContainerConnectionOptions::builder(&id).build())
                .await
            {
                debug!("Cannot traefik: {}", e);
            }
        }

        Ok(())
    }

    async fn disconnect_traefik(&self, network_id: &String) -> Result<(), ShipLiftError> {
        let docker = Docker::new();

        let containers = docker
            .containers()
            .list(&ContainerListOptions::builder().build())
            .await?;
        let traefik_container_id = containers
            .into_iter()
            .find(|c| c.image.contains("traefik"))
            .map(|c| c.id);

        if let Some(id) = traefik_container_id {
            docker
                .networks()
                .get(network_id)
                .disconnect(&ContainerConnectionOptions::builder(&id).build())
                .await?;
        }

        Ok(())
    }

    async fn delete_network(&self, app_name: &String) -> Result<(), ShipLiftError> {
        let network_name = format!("{}-net", app_name);

        let docker = Docker::new();
        for n in docker
            .networks()
            .list(&Default::default())
            .await?
            .iter()
            .filter(|n| n.name == network_name)
        {
            self.disconnect_traefik(&n.id).await?;
            docker.networks().get(&n.id).delete().await?;
        }

        Ok(())
    }

    async fn start_container(
        &self,
        app_name: &String,
        network_id: &String,
        service_config: &ServiceConfig,
        container_config: &ContainerConfig,
    ) -> Result<Service, Error> {
        let docker = Docker::new();
        let containers = docker.containers();
        let images = docker.images();

        if let Image::Named { .. } = service_config.image() {
            self.pull_image(app_name, &service_config).await?;
        }

        let mut image_to_delete = None;
        if let Some(ref container_info) = self
            .get_app_container(app_name, service_config.service_name())
            .await?
        {
            let container = containers.get(&container_info.id);
            let container_details = container.inspect().await?;

            info!(
                "Removing container {:?} of review app {:?}",
                container_info, app_name
            );

            if container_details.state.running {
                container
                    .stop(Some(core::time::Duration::from_secs(10)))
                    .await?;
            }
            container.delete().await?;

            image_to_delete = Some(container_details.image);
        }

        info!(
            "Creating new review app container for {:?}: service={:?} with image={:?} ({:?})",
            app_name,
            service_config.service_name(),
            service_config.image(),
            service_config.container_type(),
        );

        let options = DockerInfrastructure::create_container_options(
            app_name,
            &service_config,
            container_config,
        );

        let container_info = containers.create(&options).await?;
        debug!("Created container: {:?}", container_info);

        self.copy_volume_data(&container_info, service_config)
            .await?;

        containers.get(&container_info.id).start().await?;
        debug!("Started container: {:?}", container_info);

        docker
            .networks()
            .get(network_id)
            .connect(
                &ContainerConnectionOptions::builder(&container_info.id)
                    .aliases(vec![service_config.service_name().as_str()])
                    .build(),
            )
            .await?;
        debug!(
            "Connected container {:?} to {:?}",
            container_info.id, network_id
        );

        let container_details = containers.get(&container_info.id).inspect().await?;

        if let Some(image) = image_to_delete {
            info!("Clean up image {:?} of app {:?}", image, app_name);
            match images.get(&image).delete().await {
                Ok(output) => {
                    for o in output {
                        debug!("{:?}", o);
                    }
                }
                Err(err) => debug!("Could not clean up image: {:?}", err),
            }
        }

        Ok(Service::try_from(&container_details)?)
    }

    fn create_container_options(
        app_name: &String,
        service_config: &ServiceConfig,
        container_config: &ContainerConfig,
    ) -> ContainerOptions {
        let mut options = ContainerOptions::builder(&service_config.image().to_string());
        if let Some(env) = service_config.env() {
            let variables = env
                .iter()
                .map(|e| format!("{}={}", e.key(), e.value().unsecure()))
                .collect::<Vec<String>>();

            options.env(variables.iter().map(|s| s.as_str()).collect::<Vec<&str>>());
        }

        let mut labels: HashMap<&str, &str> = HashMap::new();

        let traefik_frontend = format!(
            "PathPrefixStrip: /{app_name}/{service_name}/; PathPrefix:/{app_name}/{service_name}/;",
            app_name = app_name,
            service_name = service_config.service_name()
        );
        labels.insert("traefik.frontend.rule", &traefik_frontend);

        if let Some(config_labels) = service_config.labels() {
            for (k, v) in config_labels {
                labels.insert(k, v);
            }
        }

        labels.insert(APP_NAME_LABEL, app_name);
        labels.insert(SERVICE_NAME_LABEL, &service_config.service_name());
        let container_type_name = service_config.container_type().to_string();
        labels.insert(CONTAINER_TYPE_LABEL, &container_type_name);

        let replicated_env = service_config
            .env()
            .map(|env| super::replicated_environment_variable_to_json(env))
            .flatten()
            .map(|value| value.to_string());

        if let Some(replicated_env) = &replicated_env {
            labels.insert(REPLICATED_ENV_LABEL, &replicated_env);
        }

        options.labels(&labels);
        options.restart_policy("always", 5);

        if let Some(memory_limit) = container_config.memory_limit() {
            options.memory(memory_limit.clone());
        }

        options.build()
    }

    async fn copy_volume_data(
        &self,
        container_info: &ContainerCreateInfo,
        service_config: &ServiceConfig,
    ) -> Result<(), ShipLiftError> {
        let volumes = match service_config.volumes() {
            None => return Ok(()),
            Some(volumes) => volumes.clone(),
        };

        debug!(
            "Copy data to container: {:?} (service = {})",
            container_info,
            service_config.service_name()
        );

        let docker = Docker::new();
        let containers = docker.containers();

        for (path, data) in volumes.into_iter() {
            containers
                .get(&container_info.id)
                .copy_file_into(path, &data.as_bytes())
                .await?;
        }

        Ok(())
    }

    async fn pull_image(
        &self,
        app_name: &String,
        config: &ServiceConfig,
    ) -> Result<(), ShipLiftError> {
        let image = config.image().to_string();

        info!(
            "Pulling {:?} for {:?} of app {:?}",
            image,
            config.service_name(),
            app_name
        );

        let pull_options = PullOptions::builder().image(image).build();

        let docker = Docker::new();
        let images = docker.images();
        let pull_results = images
            .pull(&pull_options)
            .collect::<Vec<Result<serde_json::Value, ShipLiftError>>>()
            .await;

        for pull_result in pull_results {
            debug!("{:?}", pull_result);
        }

        Ok(())
    }

    async fn get_app_container(
        &self,
        app_name: &String,
        service_name: &String,
    ) -> Result<Option<Container>, ShipLiftError> {
        let docker = Docker::new();
        let containers = docker.containers();

        let list_options = ContainerListOptions::builder().all().build();

        Ok(containers
            .list(&list_options)
            .await?
            .into_iter()
            .filter(|c| match c.labels.get(APP_NAME_LABEL) {
                None => false,
                Some(app) => app == app_name,
            })
            .filter(|c| match c.labels.get(SERVICE_NAME_LABEL) {
                None => false,
                Some(service) => service == service_name,
            })
            .next())
    }

    async fn get_container_details(
        &self,
        app_name: Option<&String>,
    ) -> Result<MultiMap<String, ContainerDetails>, Error> {
        debug!("Resolve container details for app {:?}", app_name);

        let f = match app_name {
            None => ContainerFilter::LabelName(String::from(APP_NAME_LABEL)),
            Some(app_name) => {
                ContainerFilter::Label(String::from(APP_NAME_LABEL), app_name.clone())
            }
        };
        let list_options = &ContainerListOptions::builder()
            .all()
            .filter(vec![f])
            .build();

        let docker = Docker::new();
        let containers = docker.containers();

        let container_list = containers.list(&list_options).await?;
        let number_of_containers = container_list.len();
        if number_of_containers == 0 {
            return Ok(MultiMap::new());
        }

        let (tx, mut rx) = mpsc::channel(container_list.len());
        for container in container_list.into_iter() {
            let mut tx = tx.clone();
            tokio::spawn(async move {
                let inspect_result = inspect(container).await;
                if let Err(err) = tx.send(inspect_result).await {
                    error!("Cannot send container inspection: {:?}", err);
                }
            });
        }

        let mut container_details = MultiMap::new();
        for _c in 0..number_of_containers {
            let details = rx.recv().await.unwrap()?;
            let app_name = details
                .config
                .labels
                .clone()
                .unwrap()
                .get(APP_NAME_LABEL)
                .unwrap()
                .clone();

            container_details.insert(app_name, details);
        }

        Ok(container_details)
    }
}

#[async_trait]
impl Infrastructure for DockerInfrastructure {
    async fn get_services(&self) -> Result<MultiMap<String, Service>, Error> {
        let mut apps = MultiMap::new();
        let container_details = self.get_container_details(None).await?;

        for (app_name, details_vec) in container_details.iter_all() {
            for details in details_vec {
                let service = match Service::try_from(details) {
                    Ok(service) => service,
                    Err(e) => {
                        debug!("Container does not provide required data: {:?}", e);
                        continue;
                    }
                };

                apps.insert(app_name.clone(), service);
            }
        }

        Ok(apps)
    }

    async fn deploy_services(
        &self,
        app_name: &String,
        configs: &Vec<ServiceConfig>,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
        let network_id = self.create_or_get_network_id(app_name).await?;

        self.connect_traefik(&network_id).await?;

        let futures = configs
            .iter()
            .map(|service_config| {
                self.start_container(app_name, &network_id, &service_config, container_config)
            })
            .collect::<Vec<_>>();

        let mut services: Vec<Service> = Vec::new();
        for service in join_all(futures).await {
            services.push(service?);
        }

        Ok(services)
    }

    /// Deletes all services for the given `app_name`.
    async fn stop_services(&self, app_name: &String) -> Result<Vec<Service>, Error> {
        let container_details = match self
            .get_container_details(Some(app_name))
            .await?
            .get_vec(app_name)
        {
            None => return Ok(vec![]),
            Some(services) => services.clone(),
        };

        let futures = container_details
            .iter()
            .filter(|details| details.state.running)
            .map(|details| stop(details.clone()));
        for container in join_all(futures).await {
            trace!("Stopped container {:?}", container?);
        }

        let mut services = Vec::with_capacity(container_details.len());
        let futures = container_details
            .iter()
            .map(|details| delete(details.clone()));
        for container in join_all(futures).await {
            let container = container?;
            trace!("Deleted container {:?}", container);

            services.push(Service::try_from(&container)?);
        }

        self.delete_network(app_name).await?;

        Ok(services)
    }

    async fn get_logs(
        &self,
        app_name: &String,
        service_name: &String,
        from: &Option<DateTime<FixedOffset>>,
        limit: usize,
    ) -> Result<Option<Vec<(DateTime<FixedOffset>, String)>>, failure::Error> {
        match self.get_app_container(app_name, service_name).await? {
            None => Ok(None),
            Some(container) => {
                let docker = Docker::new();

                trace!(
                    "Acquiring logs of container {} since {:?}",
                    container.id,
                    from
                );

                let log_options = match from {
                    Some(from) => LogsOptions::builder()
                        .since(from)
                        .stdout(true)
                        .stderr(true)
                        .timestamps(true)
                        .build(),
                    None => LogsOptions::builder()
                        .stdout(true)
                        .stderr(true)
                        .timestamps(true)
                        .build(),
                };

                let logs = docker
                    .containers()
                    .get(&container.id)
                    .logs(&log_options)
                    .collect::<Vec<Result<TtyChunk, ShipLiftError>>>()
                    .await;

                let logs = logs.into_iter()
                    .enumerate()
                    // Unfortunately, docker API does not support head (cf. https://github.com/moby/moby/issues/13096)
                    // Until then we have to skip these log messages which is super slow…
                    .filter(move |(index, _)| index < &limit)
                    .filter_map(|(_, chunk)| chunk.ok())
                    .map(|chunk| {
                        let line = String::from_utf8_lossy(&chunk.to_vec()).to_string();

                        let mut iter = line.splitn(2, ' ').into_iter();
                        let timestamp = iter.next()
                            .expect("This should never happen: docker should return timestamps, separated by space");

                        let datetime = DateTime::parse_from_rfc3339(&timestamp).expect("Expecting a valid timestamp");

                        let log_line : String = iter
                            .collect::<Vec<&str>>()
                            .join(" ");
                        (datetime, log_line)
                    })
                    .filter(move |(timestamp, _)| {
                        // Due to the fact that docker's REST API supports only unix time (cf. since),
                        // it is necessary to filter the timestamps as well.
                        from.map(|from| timestamp >= &from).unwrap_or_else(|| true)
                    })
                    .collect();

                Ok(Some(logs))
            }
        }
    }

    async fn change_status(
        &self,
        app_name: &String,
        service_name: &String,
        status: ServiceStatus,
    ) -> Result<Option<Service>, failure::Error> {
        match self.get_app_container(app_name, service_name).await? {
            Some(container) => {
                let docker = Docker::new();
                let containers = docker.containers();
                let c = containers.get(&container.id);

                let details = c.inspect().await?;

                macro_rules! run_future_and_map_err {
                    ( $future:expr, $log_format:expr ) => {
                        if let Err(err) = $future.await {
                            match err {
                                ShipLiftError::Fault { code, message } if code.as_u16() == 304 => {
                                    trace!(
                                        "Container {} already in desired state: {}",
                                        details.id,
                                        message
                                    );
                                }
                                err => {
                                    error!($log_format, err);
                                    return Err(failure::Error::from(err));
                                }
                            };
                        }
                    };
                }

                match status {
                    ServiceStatus::Running => {
                        if !details.state.running {
                            run_future_and_map_err!(c.start(), "Could not start container: {}");
                        }
                    }
                    ServiceStatus::Paused => {
                        if details.state.running {
                            run_future_and_map_err!(c.stop(None), "Could not pause container: {}");
                        }
                    }
                }

                Ok(Some(Service::try_from(&details)?))
            }
            None => Ok(None),
        }
    }
}

/// Helper function to stop containers with the aid of futures::future::join_all
async fn stop(details: ContainerDetails) -> Result<ContainerDetails, ShipLiftError> {
    let docker = Docker::new();
    let containers = docker.containers();
    containers.get(&details.id).stop(None).await?;
    Ok(details)
}

/// Helper function to delete containers with the aid of futures::future::join_all
async fn delete(details: ContainerDetails) -> Result<ContainerDetails, ShipLiftError> {
    let docker = Docker::new();
    let containers = docker.containers();
    containers.get(&details.id).delete().await?;
    Ok(details)
}

/// Helper function to inspect containers with the aid of futures::future::join_all
async fn inspect(container: Container) -> Result<ContainerDetails, ShipLiftError> {
    let docker = Docker::new();
    let containers = docker.containers();
    containers.get(&container.id).inspect().await
}

fn find_port(
    container_details: &ContainerDetails,
    labels: Option<&HashMap<String, String>>,
) -> Result<u16, DockerInfrastructureError> {
    if let Some(port) = labels
        .map(|labels| labels.get(CONTAINER_PORT_LABEL))
        .flatten()
    {
        match port.parse::<u16>() {
            Ok(port) => Ok(port),
            Err(err) => Err(DockerInfrastructureError::UnexpectedError {
                internal_message: format!(
                    "Cannot parse traefik port label into port number: {}",
                    err
                ),
            }),
        }
    } else {
        Ok(match &container_details.network_settings.ports {
            None => 80u16,
            Some(ports) => {
                let ports_regex = Regex::new(r#"^(?P<port>\d+).*"#).unwrap();
                ports
                    .keys()
                    .filter_map(|port| ports_regex.captures(port))
                    .map(|captures| String::from(captures.name("port").unwrap().as_str()))
                    .filter_map(|port| port.parse::<u16>().ok())
                    .min()
                    .unwrap_or(80u16)
            }
        })
    }
}

impl TryFrom<&ContainerDetails> for Service {
    type Error = DockerInfrastructureError;

    fn try_from(
        container_details: &ContainerDetails,
    ) -> Result<Service, DockerInfrastructureError> {
        let labels = container_details.config.labels.as_ref();

        let app_name = match labels.map(|labels| labels.get(APP_NAME_LABEL)).flatten() {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingAppNameLabel {
                    container_id: container_details.id.clone(),
                });
            }
        };

        let started_at = container_details.state.started_at.clone();
        let status = if container_details.state.running {
            ServiceStatus::Running
        } else {
            ServiceStatus::Paused
        };

        let mut builder = ServiceBuilder::new()
            .id(container_details.id.clone())
            .app_name(app_name.clone())
            .config(ServiceConfig::try_from(container_details)?)
            .service_status(status)
            .started_at(started_at);

        if !container_details.network_settings.ip_address.is_empty() {
            let addr = IpAddr::from_str(&container_details.network_settings.ip_address)?;
            let port = find_port(container_details, labels)?;
            builder = builder.endpoint(addr, port);
        }

        Ok(builder.build()?)
    }
}

impl TryFrom<&ContainerDetails> for ServiceConfig {
    type Error = DockerInfrastructureError;

    fn try_from(container_details: &ContainerDetails) -> Result<Self, Self::Error> {
        let labels = container_details.config.labels.as_ref();

        let service_name = match labels
            .map(|labels| labels.get(SERVICE_NAME_LABEL))
            .flatten()
        {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingServiceNameLabel {
                    container_id: container_details.id.clone(),
                });
            }
        };

        let image = Image::from_str(&container_details.image).unwrap();
        let mut config = ServiceConfig::new(service_name.clone(), image);

        if let Some(lb) = labels
            .map(|labels| labels.get(CONTAINER_TYPE_LABEL))
            .flatten()
        {
            config.set_container_type(lb.parse::<ContainerType>()?);
        }

        if let Some(replicated_env) = labels
            .map(|labels| labels.get(REPLICATED_ENV_LABEL))
            .flatten()
        {
            let env = serde_json::from_str::<Environment>(replicated_env).map_err(|err| {
                DockerInfrastructureError::UnexpectedError {
                    internal_message: err.to_string(),
                }
            })?;
            config.set_env(Some(env));
        }

        Ok(config)
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

impl From<AddrParseError> for DockerInfrastructureError {
    fn from(err: AddrParseError) -> Self {
        DockerInfrastructureError::InvalidContainerAddress {
            internal_message: err.to_string(),
        }
    }
}

impl From<ServiceBuilderError> for DockerInfrastructureError {
    fn from(err: ServiceBuilderError) -> Self {
        DockerInfrastructureError::UnexpectedError {
            internal_message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Environment, EnvironmentVariable};
    use crate::sc;
    use secstr::SecUtf8;

    macro_rules! container_details {
        ($id:expr, $app_name:expr, $service_name:expr, $image:expr, $container_type:expr, $($l_key:expr => $l_value:expr),* ) => {{
            let mut labels = std::collections::HashMap::new();

            if let Some(app_name) = $app_name {
                labels.insert(String::from(APP_NAME_LABEL), app_name);
            }
            if let Some(service_name) = $service_name {
                labels.insert(String::from(SERVICE_NAME_LABEL), service_name);
            }
            if let Some(container_type) = $container_type {
                labels.insert(String::from(CONTAINER_TYPE_LABEL), container_type);
            }

            $( labels.insert($l_key, $l_value); )*

            shiplift::rep::ContainerDetails {
                app_armor_profile: "".to_string(),
                args: vec![],
                config: shiplift::rep::Config {
                    attach_stderr: false,
                    attach_stdin: false,
                    attach_stdout: false,
                    cmd: None,
                    domainname: "".to_string(),
                    entrypoint: None,
                    env: None,
                    exposed_ports: None,
                    hostname: "".to_string(),
                    image: "".to_string(),
                    labels: Some(labels),
                    on_build: None,
                    open_stdin: false,
                    stdin_once: false,
                    tty: false,
                    user: "".to_string(),
                    working_dir: "".to_string(),
                },
                driver: "".to_string(),
                host_config: shiplift::rep::HostConfig {
                    cgroup_parent: None,
                    container_id_file: "".to_string(),
                    cpu_shares: None,
                    cpuset_cpus: None,
                    memory: None,
                    memory_swap: None,
                    network_mode: "".to_string(),
                    pid_mode: None,
                    port_bindings: None,
                    privileged: false,
                    publish_all_ports: false,
                    readonly_rootfs: None,
                },
                hostname_path: "".to_string(),
                hosts_path: "".to_string(),
                log_path: "".to_string(),
                id: $id,
                image: $image.unwrap_or_else(|| "".to_string()),
                mount_label: "".to_string(),
                name: "".to_string(),
                network_settings: shiplift::rep::NetworkSettings {
                    bridge: "".to_string(),
                    gateway: "".to_string(),
                    ip_address: "".to_string(),
                    ip_prefix_len: 0,
                    mac_address: "".to_string(),
                    ports: None,
                    networks: Default::default(),
                },
                path: "".to_string(),
                process_label: "".to_string(),
                resolv_conf_path: "".to_string(),
                restart_count: 0,
                state: shiplift::rep::State {
                    error: "".to_string(),
                    exit_code: 0,
                    oom_killed: false,
                    paused: false,
                    pid: 0,
                    restarting: false,
                    running: false,
                    finished_at: chrono::Utc::now(),
                    started_at: chrono::Utc::now(),
                },
                created: chrono::Utc::now(),
                mounts: vec![],
            }
        }};
    }

    #[test]
    fn should_create_container_options() {
        let config = sc!("db", "mariadb:10.3.17");

        let options = DockerInfrastructure::create_container_options(
            &String::from("master"),
            &config,
            &ContainerConfig::default(),
        );

        let json = serde_json::to_value(&options).unwrap();
        assert_json_diff::assert_json_eq!(
            json,
            serde_json::json!({
              "name": null,
              "params": {
                "HostConfig.RestartPolicy.Name": "always",
                "Image": "docker.io/library/mariadb:10.3.17",
                "Labels": {
                  "com.aixigo.preview.servant.app-name": "master",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db",
                  "traefik.frontend.rule": "PathPrefixStrip: /master/db/; PathPrefix:/master/db/;"
                }
              }
            })
        );
    }

    #[test]
    fn should_create_container_options_with_environment_variable() {
        let mut config = sc!("db", "mariadb:10.3.17");
        config.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("MYSQL_ROOT_PASSWORD"),
            SecUtf8::from("example"),
        )])));

        let options = DockerInfrastructure::create_container_options(
            &String::from("master"),
            &config,
            &ContainerConfig::default(),
        );

        let json = serde_json::to_value(&options).unwrap();
        assert_json_diff::assert_json_eq!(
            json,
            serde_json::json!({
              "name": null,
              "params": {
                "HostConfig.RestartPolicy.Name": "always",
                "Image": "docker.io/library/mariadb:10.3.17",
                "Labels": {
                  "com.aixigo.preview.servant.app-name": "master",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db",
                  "traefik.frontend.rule": "PathPrefixStrip: /master/db/; PathPrefix:/master/db/;"
                },
                "Env": [
                  "MYSQL_ROOT_PASSWORD=example"
                ]
              }
            })
        );
    }

    #[test]
    fn should_create_container_options_with_replicated_environment_variable() {
        let mut config = sc!("db", "mariadb:10.3.17");
        config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_replicated(
                String::from("MYSQL_ROOT_PASSWORD"),
                SecUtf8::from("example"),
            ),
        ])));

        let options = DockerInfrastructure::create_container_options(
            &String::from("master"),
            &config,
            &ContainerConfig::default(),
        );

        let json = serde_json::to_value(&options).unwrap();
        assert_json_diff::assert_json_eq!(
            json,
            serde_json::json!({
                "name": null,
                "params": {
                  "HostConfig.RestartPolicy.Name": "always",
                  "Image": "docker.io/library/mariadb:10.3.17",
                  "Labels": {
                    "com.aixigo.preview.servant.app-name": "master",
                    "com.aixigo.preview.servant.container-type": "instance",
                    "com.aixigo.preview.servant.service-name": "db",
                    "com.aixigo.preview.servant.replicated-env": serde_json::json!({
                      "MYSQL_ROOT_PASSWORD": {
                        "value": "example",
                        "templated": false,
                        "replicate": true,
                      }
                    }).to_string(),
                    "traefik.frontend.rule": "PathPrefixStrip: /master/db/; PathPrefix:/master/db/;"
                  },
                  "Env": [
                    "MYSQL_ROOT_PASSWORD=example"
                  ]
                }
            })
        );
    }

    #[test]
    fn should_create_service_config_from_container_details() {
        let details = container_details!(
            "some-random-id".to_string(),
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            None,
        );

        let service = Service::try_from(&details).unwrap();

        assert_eq!(service.id(), "some-random-id");
        assert_eq!(service.app_name(), "master");
        assert_eq!(service.config().service_name(), "nginx");
        assert_eq!(
            &service.config().image().to_string(),
            "docker.io/library/nginx:latest"
        );
    }

    #[test]
    fn should_create_service_config_from_container_details_with_replicated_env() {
        let details = container_details!(
            "some-random-id".to_string(),
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            None,
            String::from(REPLICATED_ENV_LABEL) => serde_json::json!({ "MYSQL_ROOT_PASSWORD": { "value": "example" } }).to_string()
        );

        let service = Service::try_from(&details).unwrap();

        assert_eq!(
            service.config().env().unwrap().get(0).unwrap(),
            &EnvironmentVariable::with_replicated(
                String::from("MYSQL_ROOT_PASSWORD"),
                SecUtf8::from("example")
            )
        );
    }
}
