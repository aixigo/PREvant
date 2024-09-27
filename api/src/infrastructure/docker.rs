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

use crate::config::{Config, ContainerConfig};
use crate::deployment::deployment_unit::{DeployableService, DeploymentStrategy};
use crate::deployment::DeploymentUnit;
use crate::infrastructure::{
    HttpForwarder, Infrastructure, APP_NAME_LABEL, CONTAINER_TYPE_LABEL, IMAGE_LABEL,
    REPLICATED_ENV_LABEL, SERVICE_NAME_LABEL, STATUS_ID,
};
use crate::models::service::{ContainerType, Service, ServiceError, ServiceStatus};
use crate::models::{
    AppName, Environment, Image, ServiceBuilder, ServiceBuilderError, ServiceConfig, WebHostMeta,
};
use anyhow::{anyhow, Result};
use async_stream::stream;
use async_trait::async_trait;
use bollard::auth::DockerCredentials;
use bollard::container::{
    CreateContainerOptions, ListContainersOptions, LogOutput, StartContainerOptions,
    UploadToContainerOptions,
};
use bollard::errors::Error as BollardError;
use bollard::image::CreateImageOptions;
use bollard::network::{
    ConnectNetworkOptions, CreateNetworkOptions, DisconnectNetworkOptions, ListNetworksOptions,
};
use bollard::secret::Port;
use bollard::service::{
    ContainerCreateResponse, ContainerInspectResponse, ContainerStateStatusEnum, ContainerSummary,
    CreateImageInfo, EndpointSettings, HostConfig, RestartPolicy, RestartPolicyNameEnum,
    VolumeListResponse,
};
use bollard::volume::{CreateVolumeOptions, ListVolumesOptions};
use bollard::Docker;
use chrono::{DateTime, FixedOffset};
use futures::stream::BoxStream;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use multimap::MultiMap;
use rocket::form::validate::Contains;
use std::collections::HashMap;
use std::convert::{From, TryFrom};
use std::str::FromStr;
use tokio::net::TcpStream;

static CONTAINER_PORT_LABEL: &str = "traefik.port";

pub struct DockerInfrastructure {
    config: Config,
}

#[derive(Debug, thiserror::Error)]
pub enum DockerInfrastructureError {
    #[error("Could not find image: {internal_message}")]
    ImageNotFound { internal_message: String },
    #[error("The container {container_id} does not provide a label for service name.")]
    MissingServiceNameLabel { container_id: String },
    #[error("The container {container_id} does not provide a label for app name.")]
    MissingAppNameLabel { container_id: String },
    #[error("Unexpected image format for image “{img}” ({err}).")]
    UnexpectedImageFormat { img: String, err: anyhow::Error },
    #[error("Unexpected docker interaction error: {err}")]
    UnexpectedError { err: anyhow::Error },
    #[error("Unknown service type label: {unknown_label}")]
    UnknownServiceType { unknown_label: String },
    #[error("Unexpected state for container: {container_id}")]
    InvalidContainerState { container_id: String },
    #[error("Unexpected image details for container: {container_id}")]
    InvalidContainerImage { container_id: String },
}

impl DockerInfrastructure {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    async fn find_status_change_container(
        &self,
        status_id: &str,
    ) -> Result<Option<ContainerSummary>, BollardError> {
        self.get_status_change_containers(None, Some(status_id))
            .await
            .map(|list| list.into_iter().next())
    }

    async fn create_status_change_container(
        &self,
        status_id: &str,
        app_name: &AppName,
    ) -> Result<ContainerInspectResponse> {
        let existing_task = self
            .get_status_change_containers(Some(app_name), None)
            .await?
            .into_iter()
            .next();

        if let Some(existing_task) = existing_task {
            // TODO: what to if there is already a deployment
            return Err(anyhow!(
                "There is already an operation in progress: {existing_task:?}"
            ));
        }

        let image = Image::from_str("docker.io/library/busybox:stable").unwrap();

        pull(&image, &self.config).await?;

        let mut labels: HashMap<&str, &str> = HashMap::new();
        labels.insert(APP_NAME_LABEL, app_name);
        labels.insert(STATUS_ID, status_id);

        let docker = Docker::connect_with_socket_defaults()?;

        trace!("Create deployment task container {status_id} for {app_name}");

        let container_info = docker
            .create_container(
                None::<CreateContainerOptions<&str>>,
                bollard::container::Config::<&str> {
                    image: Some("docker.io/library/busybox:stable"),
                    labels: Some(labels),
                    ..Default::default()
                },
            )
            .await?;
        Ok(docker.inspect_container(&container_info.id, None).await?)
    }

    async fn create_or_get_network_id(&self, app_name: &AppName) -> Result<String, BollardError> {
        trace!("Resolve network id for {app_name}");

        let network_name = format!("{app_name}-net");

        let docker = Docker::connect_with_socket_defaults()?;
        let network_id = docker
            .list_networks(None::<ListNetworksOptions<&str>>)
            .await?
            .into_iter()
            .find(|n| n.name.as_ref() == Some(&network_name))
            .and_then(|n| n.id);

        if let Some(n) = network_id {
            return Ok(n);
        }

        debug!("Creating network for app {app_name}.");

        let network_create_info = docker
            .create_network(CreateNetworkOptions::<&str> {
                name: &network_name,
                ..Default::default()
            })
            .await?;
        let network_id = network_create_info
            .id
            .expect("id is mandatory for a Docker Network.");

        debug!("Created network for app {app_name} with id {network_id}");

        Ok(network_id)
    }

    async fn connect_traefik(&self, network_id: &str) -> Result<(), BollardError> {
        let docker = Docker::connect_with_socket_defaults()?;

        let containers = docker
            .list_containers(None::<ListContainersOptions<&str>>)
            .await?;

        let traefik_container_id = containers
            .into_iter()
            .find(|c| c.image.as_ref().map_or(false, |s| s.contains("traefik")))
            .and_then(|c| c.id);

        if let Some(id) = traefik_container_id {
            if let Err(e) = docker
                .connect_network(
                    network_id,
                    ConnectNetworkOptions::<&str> {
                        container: &id,
                        ..Default::default()
                    },
                )
                .await
            {
                debug!("Cannot traefik: {e}");
            }
        }

        Ok(())
    }

    async fn disconnect_traefik(&self, network_id: &str) -> Result<(), BollardError> {
        let docker = Docker::connect_with_socket_defaults()?;
        let containers = docker
            .list_containers(None::<ListContainersOptions<&str>>)
            .await?;
        let traefik_container_id = containers
            .into_iter()
            .find(|c| c.image.as_ref().map_or(false, |s| s.contains("traefik")))
            .and_then(|c| c.id);

        if let Some(id) = traefik_container_id {
            docker
                .disconnect_network(
                    network_id,
                    DisconnectNetworkOptions::<&str> {
                        container: &id,
                        ..Default::default()
                    },
                )
                .await?
        }

        Ok(())
    }

    async fn delete_network(&self, app_name: &AppName) -> Result<(), BollardError> {
        let network_name = format!("{app_name}-net");

        let docker = Docker::connect_with_socket_defaults()?;

        for n in docker
            .list_networks(Some(ListNetworksOptions::<&str> {
                filters: HashMap::from([("name", vec![network_name.as_str()])]),
            }))
            .await?
        {
            let network_id =
                n.id.as_ref()
                    .expect("id is mandatory for a Docker Network.");
            self.disconnect_traefik(network_id).await?;
            docker.remove_network(network_id).await?;
        }

        Ok(())
    }

    async fn delete_volume_mount(&self, app_name: &AppName) -> Result<(), BollardError> {
        let docker = Docker::connect_with_socket_defaults()?;
        for volume in Self::fetch_existing_volumes(app_name)
            .await?
            .volumes
            .into_iter()
            .flatten()
        {
            docker.remove_volume(&volume.name, None).await?;
        }

        Ok(())
    }

    async fn deploy_services_impl(
        &self,
        deployment_unit: &DeploymentUnit,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, DockerInfrastructureError> {
        let app_name = deployment_unit.app_name();
        let services = deployment_unit.services();
        let network_id = self.create_or_get_network_id(app_name).await?;

        self.connect_traefik(&network_id).await?;
        let existing_volumes = Self::fetch_existing_volumes(app_name).await?;
        let mut futures = services
            .iter()
            .map(|service| {
                self.start_container(
                    app_name,
                    &network_id,
                    service,
                    container_config,
                    &existing_volumes,
                )
            })
            .map(Box::pin)
            .collect::<FuturesUnordered<_>>();

        let mut services: Vec<Service> = Vec::new();
        while let Some(service) = futures.next().await {
            services.push(service?);
        }

        Ok(services)
    }

    async fn stop_services_impl(
        &self,
        app_name: &AppName,
    ) -> Result<Vec<Service>, DockerInfrastructureError> {
        let container_details = match self
            .get_container_details(Some(app_name), None)
            .await?
            .get_vec(app_name)
        {
            None => return Ok(vec![]),
            Some(services) => services.clone(),
        };

        let docker = Docker::connect_with_socket_defaults()?;

        let mut futures = container_details
            .clone()
            .into_iter()
            .filter(|p| {
                p.state.as_ref().and_then(|state| state.status)
                    == Some(ContainerStateStatusEnum::RUNNING)
            })
            .map(|details| async {
                let id = details
                    .id
                    .as_ref()
                    .expect("id is mandatory for a docker container");

                docker.stop_container(id, None).await?;

                Ok::<ContainerInspectResponse, BollardError>(details)
            })
            .map(Box::pin)
            .collect::<FuturesUnordered<_>>();

        while let Some(result) = futures.next().await {
            let container = result?;
            let id = container
                .id
                .expect("id is mandatory for a docker container");
            trace!("Stopped container {id} for {app_name}");
        }

        let mut futures = container_details
            .into_iter()
            .map(|details| async {
                let id = details
                    .id
                    .as_ref()
                    .expect("id is mandatory for a docker container");

                docker.remove_container(id, None).await?;
                trace!("Deleted container {id} for {app_name}");

                Ok::<ContainerInspectResponse, BollardError>(details)
            })
            .map(Box::pin)
            .collect::<FuturesUnordered<_>>();

        let mut services = Vec::with_capacity(futures.len());
        while let Some(result) = futures.next().await {
            let container = result?;
            services.push(Service::try_from(container)?);
        }

        self.delete_network(app_name).await?;
        self.delete_volume_mount(app_name).await?;

        Ok(services)
    }

    async fn start_container(
        &self,
        app_name: &AppName,
        network_id: &str,
        service: &DeployableService,
        container_config: &ContainerConfig,
        existing_volumes: &VolumeListResponse,
    ) -> Result<Service, DockerInfrastructureError> {
        let docker = Docker::connect_with_socket_defaults()?;
        let service_name = service.service_name();
        let service_image = service.image();

        if let Image::Named { .. } = service_image {
            self.pull_image(app_name, service).await?;
        }
        let mut image_to_delete = None;
        if let Some(ref container_info) = Self::get_app_container(app_name, service_name).await? {
            let container_details = docker
                .inspect_container(
                    container_info
                        .id
                        .as_ref()
                        .expect("id is mandatory for a docker container"),
                    None,
                )
                .await?;

            match service.strategy() {
                DeploymentStrategy::RedeployOnImageUpdate(image_id)
                    if container_details.image.as_ref() == Some(image_id) =>
                {
                    debug!("Container {container_info:?} of review app {app_name:?} is still running with the desired image id {image_id}");
                    return Service::try_from(container_details);
                }
                DeploymentStrategy::RedeployNever => {
                    debug!(
                        "Container {container_info:?} of review app {app_name:?} already deployed."
                    );
                    return Service::try_from(container_details);
                }
                DeploymentStrategy::RedeployAlways
                | DeploymentStrategy::RedeployOnImageUpdate(_) => {}
            };

            info!("Removing container {container_info:?} of review app {app_name:?}");

            if container_details
                .state
                .map(|state| state.running == Some(true))
                .is_some()
            {
                docker
                    .stop_container(
                        container_details
                            .id
                            .as_ref()
                            .expect("id is mandatory for a docker container"),
                        None,
                    )
                    .await?;
            }
            docker
                .remove_container(
                    container_details
                        .id
                        .as_ref()
                        .expect("id is mandatory for a docker container"),
                    None,
                )
                .await?;
            image_to_delete = container_details.image;
        }

        info!(
            "Creating new review app container for {app_name:?}: service={service_name:?} with image={service_image:?} ({:?})",
            service.container_type(),
        );

        let host_config_binds =
            Self::create_host_config_binds(app_name, existing_volumes, service).await?;

        let options =
            Self::create_container_options(app_name, service, container_config, &host_config_binds);

        let container_info = docker
            .create_container::<&str, String>(None, options)
            .await?;
        let container_id = container_info.id.as_ref();

        debug!("Created container: {container_info:?}");
        self.copy_file_data(&container_info, service).await?;

        docker
            .start_container(container_id, None::<StartContainerOptions<&str>>)
            .await?;
        debug!("Started container: {container_info:?}");

        docker
            .connect_network(
                network_id,
                ConnectNetworkOptions::<&str> {
                    container: container_id,
                    endpoint_config: EndpointSettings {
                        aliases: Some(vec![service_name.to_string()]),
                        ..Default::default()
                    },
                },
            )
            .await?;

        debug!("Connected container {container_id} to {network_id}");

        let container_details = docker.inspect_container(container_id, None).await?;

        if let Some(image) = image_to_delete {
            info!("Clean up image {image:?} of app {app_name:?}");
            match docker.remove_image(&image, None, None).await {
                Ok(output) => {
                    for o in output {
                        debug!("{o:?}");
                    }
                }
                Err(err) => debug!("Could not clean up image: {err:?}"),
            };
        }
        Service::try_from(container_details)
    }

    fn create_container_options<'a>(
        app_name: &'a str,
        service_config: &'a ServiceConfig,
        container_config: &'a ContainerConfig,
        host_config_binds: &'a [String],
    ) -> bollard::container::Config<String> {
        let env = service_config.env().map(|env| {
            env.iter()
                .map(|v| format!("{}={}", v.key(), v.value().unsecure()))
                .collect::<Vec<String>>()
        });

        let mut labels: HashMap<String, String> = HashMap::new();

        let traefik_frontend = format!(
            "PathPrefixStrip: /{app_name}/{service_name}/; PathPrefix:/{app_name}/{service_name}/;",
            app_name = app_name,
            service_name = service_config.service_name()
        );
        labels.insert("traefik.frontend.rule".to_string(), traefik_frontend);

        if let Some(config_labels) = service_config.labels() {
            for (k, v) in config_labels {
                labels.insert(k.to_string(), v.to_string());
            }
        }

        labels.insert(APP_NAME_LABEL.to_string(), app_name.to_string());
        labels.insert(
            SERVICE_NAME_LABEL.to_string(),
            service_config.service_name().to_string(),
        );
        let container_type = service_config.container_type().to_string();
        labels.insert(CONTAINER_TYPE_LABEL.to_string(), container_type);
        let image_name = service_config.image().to_string();
        labels.insert(IMAGE_LABEL.to_string(), image_name);

        let replicated_env = service_config
            .env()
            .and_then(super::replicated_environment_variable_to_json)
            .map(|value| value.to_string());

        if let Some(replicated_env) = replicated_env {
            labels.insert(REPLICATED_ENV_LABEL.to_string(), replicated_env);
        }

        let memory = container_config
            .memory_limit()
            .map(|mem| mem.as_u64() as i64);

        bollard::container::Config {
            image: Some(service_config.image().to_string()),
            env,
            labels: Some(labels),
            host_config: Some(HostConfig {
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::ALWAYS),
                    ..Default::default()
                }),
                binds: Some(host_config_binds.to_vec()),
                memory,
                memory_swap: memory,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    async fn copy_file_data(
        &self,
        container_info: &ContainerCreateResponse,
        service_config: &ServiceConfig,
    ) -> Result<(), BollardError> {
        let files = match service_config.files() {
            None => return Ok(()),
            Some(files) => files.clone(),
        };

        debug!(
            "Copy data to container: {container_info:?} (service = {})",
            service_config.service_name()
        );

        let docker = Docker::connect_with_socket_defaults()?;

        let mut tar_builder = tar::Builder::new(Vec::new());

        for (path, data) in files.into_iter() {
            let mut header = tar::Header::new_gnu();
            let file_contents = data.into_unsecure();
            header.set_size(file_contents.as_bytes().len() as u64);
            header.set_mode(0o644);
            tar_builder.append_data(
                &mut header,
                path.to_path_buf()
                    .iter()
                    .skip(1)
                    .collect::<std::path::PathBuf>(),
                file_contents.as_bytes(),
            )?;
        }

        docker
            .upload_to_container(
                &container_info.id,
                Some(UploadToContainerOptions {
                    path: "/",
                    ..Default::default()
                }),
                tar_builder.into_inner()?.into(),
            )
            .await?;

        Ok(())
    }

    async fn fetch_existing_volumes(
        app_name: &AppName,
    ) -> Result<VolumeListResponse, BollardError> {
        let docker = Docker::connect_with_socket_defaults()?;
        docker
            .list_volumes(Some(ListVolumesOptions {
                filters: HashMap::from([(
                    "label".to_string(),
                    vec![format!("{APP_NAME_LABEL}={app_name}")],
                )]),
            }))
            .await
    }

    async fn create_docker_volume(
        app_name: &AppName,
        service: &DeployableService,
    ) -> Result<String, BollardError> {
        let docker = Docker::connect_with_socket_defaults()?;

        let mut labels: HashMap<&str, &str> = HashMap::new();
        labels.insert(APP_NAME_LABEL, app_name);
        labels.insert(SERVICE_NAME_LABEL, service.service_name());

        docker
            .create_volume(CreateVolumeOptions {
                labels,
                ..Default::default()
            })
            .await
            .map(|vol| vol.name)
    }

    async fn create_host_config_binds(
        app_name: &AppName,
        existing_volume: &VolumeListResponse,
        service: &DeployableService,
    ) -> Result<Vec<String>, BollardError> {
        let mut host_binds = Vec::new();

        if service.declared_volumes().is_empty() {
            return Ok(host_binds);
        }

        let service_volume = existing_volume
            .volumes
            .as_ref()
            .and_then(|volume| {
                volume
                    .iter()
                    .find(|vol| vol.labels.get(SERVICE_NAME_LABEL) == Some(service.service_name()))
            })
            .map(|info| info.name.clone());

        let volume_name = match service_volume {
            Some(name) => name,
            None => Self::create_docker_volume(app_name, service).await?,
        };

        for declared_volume in service.declared_volumes() {
            host_binds.push(format!("{volume_name}:{declared_volume}"));
        }

        Ok(host_binds)
    }

    async fn pull_image(
        &self,
        app_name: &AppName,
        config: &ServiceConfig,
    ) -> Result<(), BollardError> {
        let image = config.image();

        info!(
            "Pulling {image:?} for {:?} of app {app_name:?}",
            config.service_name()
        );

        let pull_results = pull(image, &self.config).await?;

        for pull_result in pull_results {
            debug!("{pull_result:?}");
        }

        Ok(())
    }

    async fn get_containers(
        filters: HashMap<String, Vec<String>>,
    ) -> Result<Vec<ContainerSummary>, BollardError> {
        let docker = Docker::connect_with_socket_defaults()?;

        let list_options = Some(ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        });

        docker.list_containers(list_options).await
    }

    async fn get_app_containers(
        app_name: Option<&AppName>,
        service_name: Option<&str>,
    ) -> Result<Vec<ContainerSummary>, BollardError> {
        let mut filters = HashMap::new();
        if let Some(app_name_filter) =
            label_filter(APP_NAME_LABEL, app_name.map(|app_name| app_name.as_str()))
        {
            filters
                .entry("label".to_string())
                .or_insert_with(Vec::new)
                .push(app_name_filter);
        }

        if let Some(service_name_filter) = label_filter(SERVICE_NAME_LABEL, service_name) {
            filters
                .entry("label".to_string())
                .or_insert_with(Vec::new)
                .push(service_name_filter);
        }

        Self::get_containers(filters).await
    }

    async fn get_status_change_containers(
        &self,
        app_name: Option<&AppName>,
        status_id: Option<&str>,
    ) -> Result<Vec<ContainerSummary>, BollardError> {
        let mut label_filters = vec![];
        if let Some(app_name_filter) =
            label_filter(APP_NAME_LABEL, app_name.map(|app_name| app_name.as_str()))
        {
            label_filters.push(app_name_filter);
        }

        if let Some(status_id_filter) = label_filter(STATUS_ID, status_id) {
            label_filters.push(status_id_filter);
        }

        let filters = HashMap::from([("label".to_string(), label_filters)]);
        Self::get_containers(filters).await
    }

    async fn get_app_container(
        app_name: &AppName,
        service_name: &str,
    ) -> Result<Option<ContainerSummary>, BollardError> {
        Self::get_app_containers(Some(app_name), Some(service_name))
            .await
            .map(|list| list.into_iter().next())
    }

    async fn get_container_details(
        &self,
        app_name: Option<&AppName>,
        service_name: Option<&str>,
    ) -> Result<MultiMap<AppName, ContainerInspectResponse>, DockerInfrastructureError> {
        debug!("Resolve container details for app {app_name:?}");

        let container_list = Self::get_app_containers(app_name, service_name).await?;

        let mut container_details = MultiMap::new();
        for container in container_list.into_iter() {
            if let Some(details) = not_found_to_none(inspect(container).await)? {
                let app_name = match app_name {
                    Some(app_name) => app_name.clone(),
                    None => details
                        .config
                        .as_ref()
                        .and_then(|con| {
                            con.labels.as_ref().and_then(|lab| {
                                lab.get(APP_NAME_LABEL)
                                    .and_then(|app_name| AppName::from_str(app_name).ok())
                            })
                        })
                        .unwrap(),
                };
                container_details.insert(app_name, details);
            }
        }

        Ok(container_details)
    }
}

#[async_trait]
impl Infrastructure for DockerInfrastructure {
    async fn get_services(&self) -> Result<MultiMap<AppName, Service>> {
        let mut apps = MultiMap::new();
        let container_details = self.get_container_details(None, None).await?;

        for (app_name, details_vec) in container_details.into_iter() {
            for details in details_vec {
                let service = match Service::try_from(details) {
                    Ok(service) => service,
                    Err(e) => {
                        debug!("Container does not provide required data: {e:?}");
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
        status_id: &str,
        deployment_unit: &DeploymentUnit,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>> {
        let deployment_container = self
            .create_status_change_container(status_id, deployment_unit.app_name())
            .await?;

        let result = self
            .deploy_services_impl(deployment_unit, container_config)
            .await;

        delete(deployment_container).await?;

        Ok(result?)
    }

    async fn get_status_change(&self, status_id: &str) -> Result<Option<Vec<Service>>> {
        Ok(
            match self
                .find_status_change_container(status_id)
                .await?
                .as_ref()
                .and_then(|c| {
                    c.labels
                        .as_ref()
                        .and_then(|label| label.get(APP_NAME_LABEL))
                })
                .and_then(|app_name| AppName::from_str(app_name).ok())
            {
                Some(app_name) => {
                    let mut services = Vec::new();
                    if let Some(container_details) = self
                        .get_container_details(Some(&app_name), None)
                        .await?
                        .remove(&app_name)
                    {
                        for container in container_details {
                            services.push(Service::try_from(container)?);
                        }
                    }

                    Some(services)
                }
                None => None,
            },
        )
    }

    /// Deletes all services for the given `app_name`.
    async fn stop_services(&self, status_id: &str, app_name: &AppName) -> Result<Vec<Service>> {
        let deployment_container = self
            .create_status_change_container(status_id, app_name)
            .await?;

        let result = self.stop_services_impl(app_name).await;

        delete(deployment_container).await?;

        Ok(result?)
    }

    async fn get_logs<'a>(
        &'a self,
        app_name: &'a AppName,
        service_name: &'a str,
        from: &'a Option<DateTime<FixedOffset>>,
        limit: &'a Option<usize>,
        follow: bool,
    ) -> BoxStream<'a, Result<(DateTime<FixedOffset>, String)>> {
        stream! {
            match Self::
                get_app_container(&AppName::from_str(app_name).unwrap(), service_name)
                .await
            {
                Ok(None) => {}
                Ok(Some(container)) => {
                    let docker = Docker::connect_with_socket_defaults()?;
                    let container_id = container
                        .id
                        .as_ref()
                        .expect("id is mandatory for docker container");
                    trace!("Acquiring logs of container {container_id} since {from:?}");

                    let log_options = match from {
                        Some(from) => bollard::container::LogsOptions::<&str> {
                            stdout: true,
                            stderr: true,
                            since: from.timestamp(),
                            timestamps: true,
                            follow,
                            ..Default::default()
                        },
                        None => bollard::container::LogsOptions::<&str> {
                            stdout: true,
                            stderr: true,
                            timestamps: true,
                            follow,
                            ..Default::default()
                        },
                    };

                    let logs = docker.logs(container_id, Some(log_options));

                    let mut logs = match limit {
                        Some(log_limit) => Box::pin(logs.take(*log_limit))
                            as BoxStream<Result<LogOutput, BollardError>>,
                        None => Box::pin(logs) as BoxStream<Result<LogOutput, BollardError>>,
                    };

                    while let Some(result) = logs.next().await {
                        match result {
                            Ok(chunk) => {
                                let line = chunk.to_string();

                                let mut iter = line.splitn(2, ' ');
                                let timestamp = iter.next()
                                    .expect("This should never happen: docker should return timestamps, separated by space");

                                let datetime = DateTime::parse_from_rfc3339(timestamp)
                                    .expect("Expecting a valid timestamp");
                                let log_line: String = iter.collect::<Vec<&str>>().join(" ");
                                yield Ok((datetime, log_line))
                            }
                            Err(e) => yield Err(e.into()),
                        }
                    }
                }
                Err(e) => yield Err(e.into()),
            }
        }.boxed()
    }

    async fn change_status(
        &self,
        app_name: &AppName,
        service_name: &str,
        status: ServiceStatus,
    ) -> Result<Option<Service>> {
        match Self::get_app_container(app_name, service_name).await? {
            Some(container) => {
                let docker = Docker::connect_with_socket_defaults()?;
                let details = docker
                    .inspect_container(
                        container
                            .id
                            .as_ref()
                            .expect("id is mandatory for a docker container"),
                        None,
                    )
                    .await?;

                macro_rules! run_future_and_map_err {
                    ( $future:expr, $log_format:expr ) => {
                        if let Err(err) = $future.await {
                            match err {
                                BollardError::DockerResponseServerError {
                                    status_code,
                                    message,
                                    ..
                                } if status_code == 304 => {
                                    trace!(
                                        "Container {} already in desired state: {message}",
                                        details
                                            .id
                                            .as_ref()
                                            .expect("id is mandatory for a docker container")
                                    );
                                }
                                err => {
                                    error!($log_format, err);
                                    return Err(anyhow::Error::new(err));
                                }
                            };
                        }
                    };
                }

                match status {
                    ServiceStatus::Running => {
                        if !details
                            .state
                            .as_ref()
                            .map(|state| state.running.unwrap_or_default())
                            .unwrap()
                        {
                            run_future_and_map_err!(
                                docker.start_container(
                                    container
                                        .id
                                        .as_ref()
                                        .expect("id is mandatory for a docker container"),
                                    None::<StartContainerOptions::<&str>>,
                                ),
                                "Could not start container: {}"
                            );
                        }
                    }
                    ServiceStatus::Paused => {
                        if details
                            .state
                            .as_ref()
                            .map(|state| state.running.unwrap_or_default())
                            .unwrap()
                        {
                            run_future_and_map_err!(
                                docker.stop_container(
                                    container
                                        .id
                                        .as_ref()
                                        .expect("id is mandatory for a docker container"),
                                    None
                                ),
                                "Could not pause container: {}"
                            );
                        }
                    }
                }

                Ok(Some(Service::try_from(details)?))
            }
            None => Ok(None),
        }
    }

    async fn http_forwarder(&self) -> Result<Box<dyn HttpForwarder + Send>> {
        Ok(Box::new(DockerHttpForwarder {}))
    }
}

struct DockerHttpForwarder;

#[async_trait]
impl HttpForwarder for DockerHttpForwarder {
    async fn request_web_host_meta(
        &self,
        app_name: &AppName,
        service_name: &str,
        request: http::Request<http_body_util::Empty<bytes::Bytes>>,
    ) -> Result<Option<WebHostMeta>> {
        let Some(container_details) =
            DockerInfrastructure::get_app_container(app_name, service_name).await?
        else {
            return Ok(None);
        };

        let labels = container_details.labels;
        let port = find_port(
            container_details.ports.unwrap_or_default().as_slice(),
            &labels,
        )?;

        let Some(ip) = container_details
            .network_settings
            .and_then(|network_settings| {
                let ip = network_settings
                    .networks?
                    .into_iter()
                    .find_map(|(_, network)| Some(network.ip_address?))?;

                Some(ip)
            })
        else {
            return Err(anyhow::Error::msg("Found no IP address")
                .context(format!("app {app_name}, service name {service_name}")));
        };

        let stream = TcpStream::connect(format!("{ip}:{port}")).await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                warn!("Error in connection: {}", e);
            }
        });

        let (_parts, body) = sender.send_request(request).await?.into_parts();

        let body_bytes = body.collect().await?.to_bytes();

        Ok(serde_json::from_slice::<WebHostMeta>(&body_bytes).ok())
    }
}

/// Helper function to build Label Filters
fn label_filter<S>(label_name: S, label_value: Option<S>) -> Option<String>
where
    S: AsRef<str>,
{
    let label_name = label_name.as_ref().to_string();
    match label_value.as_ref().map(AsRef::as_ref) {
        None => Some(label_name.to_string()),
        Some(value) => Some(format!("{label_name}={value}")),
    }
}

/// Helper function to map ShipLift 404 errors to None
fn not_found_to_none<T>(result: Result<T, BollardError>) -> Result<Option<T>, BollardError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(BollardError::DockerResponseServerError {
            status_code: 404u16,
            ..
        }) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Helper function to pull images
async fn pull(image: &Image, config: &Config) -> Result<Vec<CreateImageInfo>, BollardError> {
    let pull_options = CreateImageOptions::<&str> {
        from_image: &image.to_string(),
        ..Default::default()
    };
    let docker_auth = if let Some(registry) = image.registry() {
        if let Some((username, password)) = config.registry_credentials(&registry) {
            Some(DockerCredentials {
                username: Some(username.to_string()),
                password: Some(password.unsecure().to_string()),
                ..Default::default()
            })
        } else {
            None
        }
    } else {
        None
    };

    let docker = Docker::connect_with_socket_defaults()?;
    docker
        .create_image(Some(pull_options), None, docker_auth)
        .try_collect()
        .await
}

/// Helper function to delete containers with the aid of futures::future::join_all
async fn delete(
    details: ContainerInspectResponse,
) -> Result<ContainerInspectResponse, BollardError> {
    let docker = Docker::connect_with_socket_defaults()?;
    docker
        .remove_container(
            details
                .id
                .as_ref()
                .expect("id is mandatory for a docker container"),
            None,
        )
        .await?;
    Ok(details)
}

/// Helper function to inspect containers with the aid of futures::future::join_all
async fn inspect(container: ContainerSummary) -> Result<ContainerInspectResponse, BollardError> {
    let docker = Docker::connect_with_socket_defaults()?;
    docker
        .inspect_container(
            &container
                .id
                .expect("id is mandatory for a docker container"),
            None,
        )
        .await
}

fn find_port(
    ports: &[Port],
    labels: &Option<HashMap<String, String>>,
) -> Result<u16, DockerInfrastructureError> {
    if let Some(port) = labels
        .as_ref()
        .and_then(|labels| labels.get(CONTAINER_PORT_LABEL))
    {
        match port.parse::<u16>() {
            Ok(port) => Ok(port),
            Err(err) => Err(DockerInfrastructureError::UnexpectedError {
                err: anyhow::Error::new(err)
                    .context("Cannot parse traefik port label into port number"),
            }),
        }
    } else {
        Ok(ports
            .iter()
            .map(|port| port.private_port)
            .min()
            .unwrap_or(80u16))
    }
}

impl TryFrom<ContainerInspectResponse> for Service {
    type Error = DockerInfrastructureError;

    fn try_from(
        container_details: ContainerInspectResponse,
    ) -> Result<Service, DockerInfrastructureError> {
        let mut labels = container_details.config.and_then(|config| config.labels);
        let container_id = container_details
            .id
            .expect("id is mandatory for a docker container");
        let app_name = match labels
            .as_mut()
            .and_then(|labels| labels.remove(APP_NAME_LABEL))
        {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingAppNameLabel { container_id });
            }
        };

        let service_name = match labels
            .as_mut()
            .and_then(|labels| labels.remove(SERVICE_NAME_LABEL))
        {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingServiceNameLabel { container_id });
            }
        };

        let image = match labels
            .as_mut()
            .and_then(|labels| labels.remove(IMAGE_LABEL))
        {
            Some(image_label) => Image::from_str(&image_label).map_err(|err| {
                DockerInfrastructureError::UnexpectedImageFormat {
                    img: image_label,
                    err: anyhow::Error::new(err),
                }
            }),
            None => {
                let Some(image) = container_details.image else {
                    return Err(DockerInfrastructureError::InvalidContainerImage { container_id });
                };
                Image::from_str(&image).map_err(|err| {
                    DockerInfrastructureError::UnexpectedImageFormat {
                        img: image,
                        err: anyhow::Error::new(err),
                    }
                })
            }
        }?;
        let mut config = ServiceConfig::new(service_name.clone(), image);

        if let Some(lb) = labels
            .as_mut()
            .and_then(|labels| labels.remove(CONTAINER_TYPE_LABEL))
        {
            config.set_container_type(lb.parse::<ContainerType>()?);
        }

        if let Some(replicated_env) = labels
            .as_mut()
            .and_then(|labels| labels.remove(REPLICATED_ENV_LABEL))
        {
            let env = serde_json::from_str::<Environment>(&replicated_env).map_err(|err| {
                DockerInfrastructureError::UnexpectedError {
                    err: anyhow::Error::new(err),
                }
            })?;
            config.set_env(Some(env));
        }

        let Some(state) = container_details.state else {
            return Err(DockerInfrastructureError::InvalidContainerState { container_id });
        };

        let started_at = state
            .started_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .expect("started_at is mandatory for a docker container");

        let status = match state.status.unwrap_or(ContainerStateStatusEnum::PAUSED) {
            ContainerStateStatusEnum::RUNNING => ServiceStatus::Running,
            _ => ServiceStatus::Paused,
        };

        Ok(ServiceBuilder::new()
            .id(container_id.clone())
            .app_name(app_name.clone())
            .config(config)
            .service_status(status)
            .started_at(started_at.into())
            .build()?)
    }
}

impl From<BollardError> for DockerInfrastructureError {
    fn from(err: BollardError) -> Self {
        match &err {
            BollardError::DockerResponseServerError {
                status_code,
                message,
            } => match status_code {
                404u16 => {
                    return DockerInfrastructureError::ImageNotFound {
                        internal_message: message.clone(),
                    }
                }
                _ => {}
            },
            _ => {}
        }
        DockerInfrastructureError::UnexpectedError {
            err: anyhow::Error::new(err),
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
                err: anyhow::Error::new(err),
            },
        }
    }
}

impl From<ServiceBuilderError> for DockerInfrastructureError {
    fn from(err: ServiceBuilderError) -> Self {
        DockerInfrastructureError::UnexpectedError {
            err: anyhow::Error::new(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Environment, EnvironmentVariable};
    use crate::sc;
    use bollard::models::ContainerState;
    use bollard::models::ContainerStateStatusEnum;
    use bollard::models::NetworkSettings;
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
            if let Some(image) = $image {
                labels.insert(String::from(IMAGE_LABEL), image);
            }

            $( labels.insert($l_key, $l_value); )*



            ContainerInspectResponse {
                app_armor_profile: Some("".to_string()),
                args: Some(vec![]),
                config: Some(bollard::service::ContainerConfig {
                    attach_stderr: Some(false),
                    attach_stdin: Some(false),
                    attach_stdout: Some(false),
                    cmd: None,
                    domainname: Some("".to_string()),
                    entrypoint: None,
                    env: None,
                    exposed_ports: None,
                    hostname: Some("".to_string()),
                    image: Some("".to_string()),
                    labels: Some(labels),
                    on_build: None,
                    open_stdin: Some(false),
                    stdin_once: Some(false),
                    tty: Some(false),
                    user: Some("".to_string()),
                    working_dir: Some("".to_string()),
                    ..Default::default()
                }),
                driver: Some("".to_string()),
                host_config: Some(HostConfig {
                    cgroup_parent: None,
                    container_id_file: Some("".to_string()),
                    cpu_shares: None,
                    cpuset_cpus: None,
                    memory: None,
                    memory_swap: None,
                    network_mode: Some("".to_string()),
                    pid_mode: None,
                    port_bindings: None,
                    privileged: Some(false),
                    publish_all_ports: Some(false),
                    readonly_rootfs: None,
                    ..Default::default()
                }),
                hostname_path: Some("".to_string()),
                hosts_path: Some("".to_string()),
                log_path: Some("".to_string()),
                id: Some($id),
                image: Some("sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913".to_string()),
                mount_label: Some("".to_string()),
                name: Some("".to_string()),
                network_settings: Some(NetworkSettings {
                    bridge: Some("".to_string()),
                    gateway: Some("".to_string()),
                    ip_address: Some("".to_string()),
                    ip_prefix_len: Some(0),
                    mac_address: Some("".to_string()),
                    ports: None,
                    networks: None,
                    ..Default::default()
                }),
                path: Some("".to_string()),
                process_label: Some("".to_string()),
                resolv_conf_path: Some("".to_string()),
                restart_count: Some(0) ,
                state: Some(ContainerState {
                    status: Some(ContainerStateStatusEnum::RUNNING),
                    error: Some("".to_string()),
                    exit_code: Some(0),
                    oom_killed: Some(false),
                    paused: Some(false),
                    pid: Some(0),
                    restarting: Some(false),
                    running: Some(false),
                    finished_at: Some(chrono::Utc::now().to_rfc3339()),
                    started_at: Some(chrono::Utc::now().to_rfc3339()),
                    ..Default::default()
                }),
                created: Some(chrono::Utc::now().to_rfc3339()),
                mounts: Some(vec![]),
                ..Default::default()
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
            &Vec::new(),
        );

        let json = serde_json::to_value(&options).unwrap();
        assert_json_diff::assert_json_eq!(
            json,
            serde_json::json!({
              "HostConfig": {
                "Binds": [],
                "RestartPolicy": {
                  "Name": "always"
                }
              },
              "Image": "docker.io/library/mariadb:10.3.17",
              "Labels": {
                "com.aixigo.preview.servant.app-name": "master",
                "com.aixigo.preview.servant.container-type": "instance",
                "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
                "com.aixigo.preview.servant.service-name": "db",
                "traefik.frontend.rule": "PathPrefixStrip: /master/db/; PathPrefix:/master/db/;"
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
            &Vec::new(),
        );

        let json = serde_json::to_value(&options).unwrap();
        assert_json_diff::assert_json_eq!(
            json,
            serde_json::json!({
              "Env": [
                "MYSQL_ROOT_PASSWORD=example"
              ],
              "HostConfig": {
                "Binds": [],
                "RestartPolicy": {
                  "Name": "always"
                }
              },
              "Image": "docker.io/library/mariadb:10.3.17",
              "Labels": {
                "com.aixigo.preview.servant.app-name": "master",
                "com.aixigo.preview.servant.container-type": "instance",
                "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
                "com.aixigo.preview.servant.service-name": "db",
                "traefik.frontend.rule": "PathPrefixStrip: /master/db/; PathPrefix:/master/db/;"
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
            &Vec::new(),
        );

        let json = serde_json::to_value(&options).unwrap();
        assert_json_diff::assert_json_eq!(
            json,
            serde_json::json!({
              "Env": [
                "MYSQL_ROOT_PASSWORD=example"
              ],
              "HostConfig": {
                "Binds": [],
                "RestartPolicy": {
                  "Name": "always"
                }
              },
              "Image": "docker.io/library/mariadb:10.3.17",
              "Labels": {
                "com.aixigo.preview.servant.app-name": "master",
                "com.aixigo.preview.servant.container-type": "instance",
                "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
                "com.aixigo.preview.servant.replicated-env": serde_json::json!({
                        "MYSQL_ROOT_PASSWORD": {
                        "value": "example",
                        "templated": false,
                        "replicate": true,
                        }
                    }).to_string(),
                "com.aixigo.preview.servant.service-name": "db",
                "traefik.frontend.rule": "PathPrefixStrip: /master/db/; PathPrefix:/master/db/;"
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

        let service = Service::try_from(details).unwrap();

        assert_eq!(service.id(), "some-random-id");
        assert_eq!(service.app_name(), "master");
        assert_eq!(service.config().service_name(), "nginx");
        assert_eq!(
            &service.config().image().to_string(),
            "docker.io/library/nginx:latest"
        );
    }

    #[test]
    fn should_not_create_service_config_from_container_details_with_invalid_image_information() {
        let details = container_details!(
            "some-random-id".to_string(),
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("\n")),
            None,
        );

        let error = Service::try_from(details).unwrap_err();
        assert!(matches!(
            error,
            DockerInfrastructureError::UnexpectedImageFormat {
                img,
                ..
            }
            if img == String::from("\n")// TODO && err == String::from("Invalid image: \n")
        ));
    }

    #[test]
    fn should_create_service_config_from_container_details_without_image_information() {
        let details = container_details!(
            "some-random-id".to_string(),
            Some(String::from("master")),
            Some(String::from("nginx")),
            None,
            None,
        );

        let service = Service::try_from(details).unwrap();

        assert_eq!(service.id(), "some-random-id");
        assert_eq!(service.app_name(), "master");
        assert_eq!(service.config().service_name(), "nginx");
        assert_eq!(
            service.config().image().to_string(),
            "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913"
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

        let service = Service::try_from(details).unwrap();

        assert_eq!(
            service.config().env().unwrap().get(0).unwrap(),
            &EnvironmentVariable::with_replicated(
                String::from("MYSQL_ROOT_PASSWORD"),
                SecUtf8::from("example")
            )
        );
    }

    #[test]
    fn should_create_container_options_with_host_config_binds() {
        let config = sc!("db", "mariadb:10.3.17");

        let options = DockerInfrastructure::create_container_options(
            &String::from("master"),
            &config,
            &ContainerConfig::default(),
            &[String::from("test-volume:/var/lib/mysql")],
        );

        let json = serde_json::to_value(&options).unwrap();
        assert_json_diff::assert_json_eq!(
            json,
            serde_json::json!({
              "HostConfig": {
                "Binds": [
                  "test-volume:/var/lib/mysql"
                ],
                "RestartPolicy": {
                  "Name": "always"
                }
              },
              "Image": "docker.io/library/mariadb:10.3.17",
              "Labels": {
                "com.aixigo.preview.servant.app-name": "master",
                "com.aixigo.preview.servant.container-type": "instance",
                "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
                "com.aixigo.preview.servant.service-name": "db",
                "traefik.frontend.rule": "PathPrefixStrip: /master/db/; PathPrefix:/master/db/;"
              }
            })
        );
    }
}
