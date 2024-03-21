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
    Infrastructure, APP_NAME_LABEL, CONTAINER_TYPE_LABEL, IMAGE_LABEL, REPLICATED_ENV_LABEL,
    SERVICE_NAME_LABEL, STATUS_ID,
};
use crate::models::service::{ContainerType, Service, ServiceError, ServiceStatus};
use crate::models::{
    AppName, Environment, Image, ServiceBuilder, ServiceBuilderError, ServiceConfig,
};
use async_stream::stream;
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use failure::{format_err, Error};
use futures::future::join_all;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use multimap::MultiMap;
use regex::Regex;
use shiplift::container::{ContainerCreateInfo, ContainerDetails, ContainerInfo};
use shiplift::errors::Error as ShipLiftError;
use shiplift::tty::TtyChunk;
use shiplift::volume::VolumeInfo;
use shiplift::{
    ContainerConnectionOptions, ContainerFilter, ContainerListOptions, ContainerOptions, Docker,
    LogsOptions, NetworkCreateOptions, PullOptions, RegistryAuth, VolumeCreateOptions,
};
use std::collections::HashMap;
use std::convert::{From, TryFrom};
use std::net::{AddrParseError, IpAddr};
use std::str::FromStr;

static CONTAINER_PORT_LABEL: &str = "traefik.port";

pub struct DockerInfrastructure {
    config: Config,
}

#[derive(Debug, Fail, PartialEq)]
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
    #[fail(display = "Unexpected image format for image “{}” ({}).", img, err)]
    UnexpectedImageFormat { img: String, err: String },
    #[fail(display = "Unexpected docker interaction error: {}", internal_message)]
    UnexpectedError { internal_message: String },
    #[fail(display = "Unknown service type label: {}", unknown_label)]
    UnknownServiceType { unknown_label: String },
    #[fail(display = "Unexpected container address: {}", internal_message)]
    InvalidContainerAddress { internal_message: String },
}

impl DockerInfrastructure {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    async fn find_status_change_container(
        &self,
        status_id: &str,
    ) -> Result<Option<ContainerInfo>, ShipLiftError> {
        self.get_status_change_containers(None, Some(status_id))
            .await
            .map(|list| list.into_iter().next())
    }

    async fn create_status_change_container(
        &self,
        status_id: &str,
        app_name: &AppName,
    ) -> Result<ContainerDetails, Error> {
        let existing_task = self
            .get_status_change_containers(Some(app_name), None)
            .await?
            .into_iter()
            .next();

        if let Some(existing_task) = existing_task {
            // TODO: what to if there is already a deployment
            return Err(format_err!(
                "There is already an operation in progress: {:?}",
                existing_task
            ));
        }

        let image = Image::from_str("docker.io/library/busybox:stable").unwrap();

        pull(&image, &self.config).await?;

        let mut labels: HashMap<&str, &str> = HashMap::new();
        labels.insert(APP_NAME_LABEL, app_name);
        labels.insert(STATUS_ID, status_id);

        let mut options = ContainerOptions::builder(&image.to_string());
        options.labels(&labels);

        let docker = Docker::new();
        let containers = docker.containers();

        trace!(
            "Create deployment task container {} for {}",
            status_id,
            app_name
        );
        let container_info = containers.create(&options.build()).await;
        let ci = container_info?;

        let container = containers.get(&ci.id);
        Ok(container.inspect().await?)
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

    async fn delete_volume_mount(&self, app_name: &String) -> Result<(), ShipLiftError> {
        let docker = Docker::new();
        let docker_volumes = docker.volumes();
        for volume in DockerInfrastructure::fetch_existing_volumes(app_name).await? {
            docker_volumes.get(&volume.name).delete().await?
        }
        Ok(())
    }

    async fn deploy_services_impl(
        &self,
        deployment_unit: &DeploymentUnit,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
        let app_name = deployment_unit.app_name();
        let services = deployment_unit.services();
        let network_id = self.create_or_get_network_id(app_name).await?;

        self.connect_traefik(&network_id).await?;
        let existing_volumes = DockerInfrastructure::fetch_existing_volumes(app_name).await?;
        let futures = services
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
            .collect::<Vec<_>>();

        let mut services: Vec<Service> = Vec::new();
        for service in join_all(futures).await {
            services.push(service?);
        }

        Ok(services)
    }

    async fn stop_services_impl(&self, app_name: &AppName) -> Result<Vec<Service>, Error> {
        let container_details = match self
            .get_container_details(Some(app_name), None)
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
        self.delete_volume_mount(app_name).await?;

        Ok(services)
    }

    async fn start_container(
        &self,
        app_name: &AppName,
        network_id: &String,
        service: &DeployableService,
        container_config: &ContainerConfig,
        existing_volumes: &[VolumeInfo],
    ) -> Result<Service, Error> {
        let docker = Docker::new();
        let containers = docker.containers();
        let images = docker.images();

        if let Image::Named { .. } = service.image() {
            self.pull_image(app_name, service).await?;
        }
        let mut image_to_delete = None;
        if let Some(ref container_info) = self
            .get_app_container(app_name, service.service_name())
            .await?
        {
            let container = containers.get(&container_info.id);
            let container_details = container.inspect().await?;

            match service.strategy() {
                DeploymentStrategy::RedeployOnImageUpdate(image_id)
                    if &container_details.image == image_id =>
                {
                    debug!("Container {:?} of review app {:?} is still running with the desired image id {}", container_info, app_name, image_id);
                    return Ok(Service::try_from(&container_details)?);
                }
                DeploymentStrategy::RedeployNever => {
                    debug!(
                        "Container {:?} of review app {:?} already deployed.",
                        container_info, app_name
                    );
                    return Ok(Service::try_from(&container_details)?);
                }
                DeploymentStrategy::RedeployAlways
                | DeploymentStrategy::RedeployOnImageUpdate(_) => {}
            };

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
            service.service_name(),
            service.image(),
            service.container_type(),
        );

        let host_config_binds =
            DockerInfrastructure::create_host_config_binds(app_name, existing_volumes, service)
                .await?;

        let options = DockerInfrastructure::create_container_options(
            app_name,
            service,
            container_config,
            &host_config_binds,
        );

        let container_info = containers.create(&options).await?;
        debug!("Created container: {:?}", container_info);

        self.copy_file_data(&container_info, service).await?;

        containers.get(&container_info.id).start().await?;
        debug!("Started container: {:?}", container_info);

        docker
            .networks()
            .get(network_id)
            .connect(
                &ContainerConnectionOptions::builder(&container_info.id)
                    .aliases(vec![service.service_name().as_str()])
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
        app_name: &str,
        service_config: &ServiceConfig,
        container_config: &ContainerConfig,
        host_config_binds: &[String],
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
        labels.insert(SERVICE_NAME_LABEL, service_config.service_name());
        let container_type_name = service_config.container_type().to_string();
        labels.insert(CONTAINER_TYPE_LABEL, &container_type_name);
        let image_name = service_config.image().to_string();
        labels.insert(IMAGE_LABEL, &image_name);

        let replicated_env = service_config
            .env()
            .and_then(|env| super::replicated_environment_variable_to_json(env))
            .map(|value| value.to_string());

        if let Some(replicated_env) = &replicated_env {
            labels.insert(REPLICATED_ENV_LABEL, replicated_env);
        }

        if !host_config_binds.is_empty() {
            options.volumes(host_config_binds.iter().map(|bind| bind.as_str()).collect());
        }
        options.labels(&labels);
        options.restart_policy("always", 5);

        if let Some(memory_limit) = container_config.memory_limit() {
            options.memory(memory_limit.as_u64());
            options.memory_swap(memory_limit.as_u64() as i64);
        }

        options.build()
    }

    async fn copy_file_data(
        &self,
        container_info: &ContainerCreateInfo,
        service_config: &ServiceConfig,
    ) -> Result<(), ShipLiftError> {
        let files = match service_config.files() {
            None => return Ok(()),
            Some(files) => files.clone(),
        };

        debug!(
            "Copy data to container: {:?} (service = {})",
            container_info,
            service_config.service_name()
        );

        let docker = Docker::new();
        let containers = docker.containers();

        for (path, data) in files.into_iter() {
            containers
                .get(&container_info.id)
                .copy_file_into(path, data.into_unsecure().as_bytes())
                .await?;
        }

        Ok(())
    }

    async fn fetch_existing_volumes(app_name: &String) -> Result<Vec<VolumeInfo>, ShipLiftError> {
        let docker = Docker::new();
        docker.volumes().list().await.map(|volume_infos| {
            volume_infos
                .into_iter()
                .filter(|s| {
                    s.labels
                        .as_ref()
                        .map(|label| label.get(APP_NAME_LABEL) == Some(app_name))
                        .unwrap_or(false)
                })
                .collect::<Vec<VolumeInfo>>()
        })
    }

    async fn create_docker_volume(
        app_name: &str,
        service: &DeployableService,
    ) -> Result<String, ShipLiftError> {
        let docker = Docker::new();
        let volumes = docker.volumes();

        let mut labels: HashMap<&str, &str> = HashMap::new();
        labels.insert(APP_NAME_LABEL, app_name);
        labels.insert(SERVICE_NAME_LABEL, service.service_name());

        let volume_options = VolumeCreateOptions::builder().labels(&labels).build();
        volumes
            .create(&volume_options)
            .await
            .map(|volume_info| volume_info.name)
    }

    async fn create_host_config_binds(
        app_name: &str,
        existing_volume: &[VolumeInfo],
        service: &DeployableService,
    ) -> Result<Vec<String>, ShipLiftError> {
        let mut host_binds = Vec::new();

        if service.declared_volumes().is_empty() {
            return Ok(host_binds);
        }

        let service_volume = existing_volume
            .iter()
            .find(|vol| {
                vol.labels.as_ref().map_or_else(
                    || false,
                    |label| label.get(SERVICE_NAME_LABEL) == Some(service.service_name()),
                )
            })
            .map(|info| &info.name);

        let volume_name = match service_volume {
            Some(name) => String::from(name),
            None => DockerInfrastructure::create_docker_volume(app_name, service).await?,
        };

        for declared_volume in service.declared_volumes() {
            host_binds.push(format!("{}:{}", volume_name, declared_volume));
        }

        Ok(host_binds)
    }

    async fn pull_image(
        &self,
        app_name: &String,
        config: &ServiceConfig,
    ) -> Result<(), ShipLiftError> {
        let image = config.image();

        info!(
            "Pulling {:?} for {:?} of app {:?}",
            image,
            config.service_name(),
            app_name
        );

        let pull_results = pull(image, &self.config).await?;

        for pull_result in pull_results {
            debug!("{:?}", pull_result);
        }

        Ok(())
    }

    async fn get_containers(
        &self,
        filters: Vec<ContainerFilter>,
    ) -> Result<Vec<ContainerInfo>, ShipLiftError> {
        let docker = Docker::new();
        let containers = docker.containers();

        let list_options = ContainerListOptions::builder()
            .all()
            .filter(filters)
            .build();

        containers.list(&list_options).await
    }

    async fn get_app_containers(
        &self,
        app_name: Option<&AppName>,
        service_name: Option<&str>,
    ) -> Result<Vec<ContainerInfo>, ShipLiftError> {
        let filters = vec![
            label_filter(APP_NAME_LABEL, app_name.map(|app_name| app_name.as_str())),
            label_filter(SERVICE_NAME_LABEL, service_name),
        ];
        self.get_containers(filters).await
    }

    async fn get_status_change_containers(
        &self,
        app_name: Option<&AppName>,
        status_id: Option<&str>,
    ) -> Result<Vec<ContainerInfo>, ShipLiftError> {
        let filters = vec![
            label_filter(APP_NAME_LABEL, app_name.map(|app_name| app_name.as_str())),
            label_filter(STATUS_ID, status_id),
        ];
        self.get_containers(filters).await
    }

    async fn get_app_container(
        &self,
        app_name: &AppName,
        service_name: &str,
    ) -> Result<Option<ContainerInfo>, ShipLiftError> {
        self.get_app_containers(Some(app_name), Some(service_name))
            .await
            .map(|list| list.into_iter().next())
    }

    async fn get_container_details(
        &self,
        app_name: Option<&AppName>,
        service_name: Option<&str>,
    ) -> Result<MultiMap<AppName, ContainerDetails>, Error> {
        debug!("Resolve container details for app {:?}", app_name);

        let container_list = self.get_app_containers(app_name, service_name).await?;

        let mut container_details = MultiMap::new();
        for container in container_list.into_iter() {
            if let Some(details) = not_found_to_none(inspect(container).await)? {
                let app_name = match app_name {
                    Some(app_name) => app_name.clone(),
                    None => details
                        .config
                        .labels
                        .clone()
                        .unwrap()
                        .get(APP_NAME_LABEL)
                        .and_then(|app_name| AppName::from_str(app_name).ok())
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
    async fn get_services(&self) -> Result<MultiMap<AppName, Service>, Error> {
        let mut apps = MultiMap::new();
        let container_details = self.get_container_details(None, None).await?;

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
        status_id: &str,
        deployment_unit: &DeploymentUnit,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
        let deployment_container = self
            .create_status_change_container(status_id, deployment_unit.app_name())
            .await?;

        let result = self
            .deploy_services_impl(deployment_unit, container_config)
            .await;

        delete(deployment_container).await?;

        result
    }

    async fn get_status_change(&self, status_id: &str) -> Result<Option<Vec<Service>>, Error> {
        Ok(
            match self
                .find_status_change_container(status_id)
                .await?
                .as_ref()
                .and_then(|c| c.labels.get(APP_NAME_LABEL))
                .and_then(|app_name| AppName::from_str(app_name).ok())
            {
                Some(app_name) => {
                    let mut services = Vec::new();
                    if let Some(container_details) = self
                        .get_container_details(Some(&app_name), None)
                        .await?
                        .get_vec(&app_name)
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
    async fn stop_services(
        &self,
        status_id: &str,
        app_name: &AppName,
    ) -> Result<Vec<Service>, Error> {
        let deployment_container = self
            .create_status_change_container(status_id, app_name)
            .await?;

        let result = self.stop_services_impl(app_name).await;

        delete(deployment_container).await?;

        result
    }

    async fn get_logs<'a>(
        &'a self,
        app_name: &'a AppName,
        service_name: &'a str,
        from: &'a Option<DateTime<FixedOffset>>,
        limit: &'a Option<usize>,
        follow: bool,
    ) -> BoxStream<'a, Result<(DateTime<FixedOffset>, String), failure::Error>> {
        stream! {
            match self
                .get_app_container(&AppName::from_str(app_name).unwrap(), service_name)
                .await
            {
                Ok(None) => {}
                Ok(Some(container)) => {
                    let docker = Docker::new();

                    trace!(
                        "Acquiring logs of container {} since {:?}",
                        container.id,
                        from
                    );

                    let mut log_options = LogsOptions::builder();
                    log_options.stdout(true).stderr(true).timestamps(true);

                    if let Some(since) = from {
                        log_options.since(since);
                    }

                    log_options.follow(follow);

                    let logs = docker
                        .containers()
                        .get(&container.id)
                        .logs(&log_options.build());

                    let mut logs = match limit {
                        Some(log_limit) => Box::pin(logs.take(*log_limit))
                            as BoxStream<Result<TtyChunk, shiplift::Error>>,
                        None => Box::pin(logs) as BoxStream<Result<TtyChunk, shiplift::Error>>,
                    };

                    while let Some(result) = logs.next().await {
                        match result {
                            Ok(chunk) => {
                                let line = String::from_utf8_lossy(&chunk.to_vec()).to_string();

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

/// Helper function to build ContainerFilters
fn label_filter<S>(label_name: S, label_value: Option<S>) -> ContainerFilter
where
    S: AsRef<str>,
{
    let label_name = label_name.as_ref().to_string();
    match label_value {
        None => ContainerFilter::LabelName(label_name),
        Some(value) => ContainerFilter::Label(label_name, value.as_ref().to_string()),
    }
}

/// Helper function to map ShipLift 404 errors to None
fn not_found_to_none<T>(result: Result<T, ShipLiftError>) -> Result<Option<T>, ShipLiftError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(ShipLiftError::Fault { code, .. }) if code.as_u16() == 404u16 => Ok(None),
        Err(err) => Err(err),
    }
}

/// Helper function to pull images
async fn pull(image: &Image, config: &Config) -> Result<Vec<serde_json::Value>, ShipLiftError> {
    let mut pull_options_builder = PullOptions::builder();
    pull_options_builder.image(&image.to_string());

    if let Some(registry) = image.registry() {
        if let Some((username, password)) = config.registry_credentials(&registry) {
            pull_options_builder.auth(
                RegistryAuth::builder()
                    .username(username)
                    .password(password.unsecure())
                    .build(),
            );
        }
    }

    let docker = Docker::new();
    let images = docker.images();

    images
        .pull(&pull_options_builder.build())
        .try_collect()
        .await
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
async fn inspect(container: ContainerInfo) -> Result<ContainerDetails, ShipLiftError> {
    let docker = Docker::new();
    let containers = docker.containers();
    containers.get(&container.id).inspect().await
}

fn find_port(
    container_details: &ContainerDetails,
    labels: Option<&HashMap<String, String>>,
) -> Result<u16, DockerInfrastructureError> {
    if let Some(port) = labels.and_then(|labels| labels.get(CONTAINER_PORT_LABEL)) {
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

        let app_name = match labels.and_then(|labels| labels.get(APP_NAME_LABEL)) {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingAppNameLabel {
                    container_id: container_details.id.clone(),
                });
            }
        };

        let started_at = container_details.state.started_at;
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

        let service_name = match labels.and_then(|labels| labels.get(SERVICE_NAME_LABEL)) {
            Some(name) => name,
            None => {
                return Err(DockerInfrastructureError::MissingServiceNameLabel {
                    container_id: container_details.id.clone(),
                });
            }
        };

        let image = match labels.and_then(|labels| labels.get(IMAGE_LABEL)) {
            Some(image_label) => Image::from_str(image_label).map_err(|err| {
                DockerInfrastructureError::UnexpectedImageFormat {
                    img: image_label.clone(),
                    err: err.to_string(),
                }
            }),
            None => Image::from_str(&container_details.image).map_err(|err| {
                DockerInfrastructureError::UnexpectedImageFormat {
                    img: container_details.image.clone(),
                    err: err.to_string(),
                }
            }),
        }?;
        let mut config = ServiceConfig::new(service_name.clone(), image);

        if let Some(lb) = labels.and_then(|labels| labels.get(CONTAINER_TYPE_LABEL)) {
            config.set_container_type(lb.parse::<ContainerType>()?);
        }

        if let Some(replicated_env) = labels.and_then(|labels| labels.get(REPLICATED_ENV_LABEL)) {
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
            if let Some(image) = $image {
                labels.insert(String::from(IMAGE_LABEL), image);
            }

            $( labels.insert($l_key, $l_value); )*

            ContainerDetails {
                app_armor_profile: "".to_string(),
                args: vec![],
                config: shiplift::image::Config {
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
                host_config: shiplift::container::HostConfig {
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
                image: "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913".to_string(),
                mount_label: "".to_string(),
                name: "".to_string(),
                network_settings: shiplift::network::NetworkSettings {
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
                state: shiplift::container::State {
                    status: "Running".to_string(),
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
            &Vec::new(),
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
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
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
            &Vec::new(),
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
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
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
            &Vec::new(),
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
                    "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
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
    fn should_not_create_service_config_from_container_details_with_invalid_image_information() {
        let details = container_details!(
            "some-random-id".to_string(),
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("\n")),
            None,
        );

        let error = Service::try_from(&details).unwrap_err();
        assert_eq!(
            error,
            DockerInfrastructureError::UnexpectedImageFormat {
                img: String::from("\n"),
                err: String::from("Invalid image: \n")
            }
        );
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

        let service = Service::try_from(&details).unwrap();

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

        let service = Service::try_from(&details).unwrap();

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
              "name": null,
              "params": {
                "HostConfig.RestartPolicy.Name": "always",
                "Image": "docker.io/library/mariadb:10.3.17",
                "HostConfig.Binds":["test-volume:/var/lib/mysql"],
                "Labels": {
                  "com.aixigo.preview.servant.app-name": "master",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db",
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
                  "traefik.frontend.rule": "PathPrefixStrip: /master/db/; PathPrefix:/master/db/;"
                }
              }
            })
        );
    }
}
