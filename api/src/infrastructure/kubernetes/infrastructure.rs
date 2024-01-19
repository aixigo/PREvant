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
use super::super::{
    APP_NAME_LABEL, CONTAINER_TYPE_LABEL, IMAGE_LABEL, REPLICATED_ENV_LABEL, SERVICE_NAME_LABEL,
    STORAGE_TYPE_LABEL,
};
use super::deployment_unit::K8sDeploymentUnit;
use super::payloads::{
    deployment_payload, image_pull_secret_payload, ingress_route_payload, middleware_payload,
    namespace_payload, persistent_volume_claim_payload, secrets_payload, service_payload,
    IngressRoute, Middleware,
};
use crate::config::{Config as PREvantConfig, ContainerConfig, Runtime};
use crate::deployment::deployment_unit::{DeployableService, DeploymentUnit};
use crate::infrastructure::traefik::TraefikIngressRoute;
use crate::infrastructure::Infrastructure;
use crate::models::service::{ContainerType, Service, ServiceError, ServiceStatus};
use crate::models::{
    AppName, Environment, Image, ServiceBuilder, ServiceBuilderError, ServiceConfig,
};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use failure::Error;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use k8s_openapi::api::storage::v1::StorageClass;
use k8s_openapi::api::{
    apps::v1::Deployment as V1Deployment, core::v1::Namespace as V1Namespace,
    core::v1::Pod as V1Pod, core::v1::Secret as V1Secret, core::v1::Service as V1Service,
};
use kube::{
    api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams},
    client::Client,
    config::Config,
    error::{Error as KubeError, ErrorResponse},
};
use log::{debug, warn};
use multimap::MultiMap;
use secstr::SecUtf8;
use std::collections::{BTreeMap, HashMap};
use std::convert::{From, TryFrom};
use std::net::IpAddr;
use std::str::FromStr;

pub struct KubernetesInfrastructure {
    config: PREvantConfig,
}

#[derive(Debug, Fail, PartialEq)]
pub enum KubernetesInfrastructureError {
    #[fail(
        display = "Unexpected Kubernetes interaction error: {}",
        internal_message
    )]
    UnexpectedError { internal_message: String },
    #[fail(
        display = "The deployment {} does not provide a label for app name.",
        deployment_name
    )]
    MissingAppNameLabel { deployment_name: String },
    #[fail(display = "Unknown service type label {}", unknown_label)]
    UnknownServiceType { unknown_label: String },
    #[fail(
        display = "The deployment {} does not provide a label for image.",
        deployment_name
    )]
    MissingImageLabel { deployment_name: String },
    #[fail(display = "The default storage class is missing in kubernetes.")]
    MissingDefaultStorageClass,
}

impl KubernetesInfrastructure {
    pub fn new(config: PREvantConfig) -> Self {
        Self { config }
    }

    async fn client(&self) -> Result<Client, KubernetesInfrastructureError> {
        let configuration = Config::infer().await.map_err(|err| {
            KubernetesInfrastructureError::UnexpectedError {
                internal_message: format!(
                    "Failed to read Kube configuration from cluster env: {err}"
                ),
            }
        })?;

        Client::try_from(configuration).map_err(|err| {
            KubernetesInfrastructureError::UnexpectedError {
                internal_message: format!("Failed to create client: {}", err),
            }
        })
    }

    async fn get_deployment_and_pod(
        &self,
        app_name: &AppName,
        service_name: &str,
    ) -> Result<Option<(V1Deployment, Option<V1Pod>)>, KubernetesInfrastructureError> {
        let client = self.client().await?;
        let namespace = app_name.to_rfc1123_namespace_id();

        let p = ListParams {
            label_selector: Some(format!("{SERVICE_NAME_LABEL}={service_name}",)),
            ..Default::default()
        };

        let client_clone = client.clone();
        let deployment = async {
            Api::<V1Deployment>::namespaced(client_clone, &namespace)
                .list(&p)
                .await
                .map(|list| list.items.into_iter().next())
        };
        let pods = async {
            Api::<V1Pod>::namespaced(client, &namespace)
                .list(&Default::default())
                .await
                .map(|list| list.items)
        };

        let (deployment, pods) = futures::try_join!(deployment, pods)?;

        Ok(deployment.and_then(|deployment| {
            let spec = deployment.spec.as_ref()?;
            let matches_labels = spec.selector.match_labels.as_ref()?;
            let pod = pods.into_iter().find(|pod| {
                pod.metadata
                    .labels
                    .as_ref()
                    .map(|labels| matches_labels.iter().all(|(k, v)| labels.get(k) == Some(v)))
                    .unwrap_or(false)
            });

            Some((deployment, pod))
        }))
    }

    fn create_service_from_deployment_and_pod(
        deployment: V1Deployment,
        pod: Option<V1Pod>,
    ) -> Result<Service, KubernetesInfrastructureError> {
        let mut builder = ServiceBuilder::try_from(deployment.clone())?;

        if let Some(pod) = pod {
            if let Some(container) = pod.spec.as_ref().and_then(|spec| spec.containers.first()) {
                builder = builder.started_at(
                    pod.status
                        .as_ref()
                        .and_then(|s| s.start_time.as_ref())
                        .map(|t| t.0)
                        .unwrap_or_else(Utc::now),
                );

                if let Some(ip) = pod.status.as_ref().and_then(|pod| pod.pod_ip.as_ref()) {
                    let port = container
                        .ports
                        .as_ref()
                        .and_then(|ports| ports.first())
                        .map(|port| port.container_port as u16)
                        .unwrap_or(80u16);

                    builder = builder.endpoint(
                        IpAddr::from_str(ip)
                            .expect("Kubernetes API should provide valid IP address"),
                        port,
                    );
                }
            }
        }

        Ok(builder.build()?)
    }

    async fn get_services_of_app(
        &self,
        app_name: &AppName,
    ) -> Result<Vec<Service>, KubernetesInfrastructureError> {
        let client = self.client().await?;

        let namespace = app_name.to_rfc1123_namespace_id();
        let list_param = Default::default();
        let client_clone = client.clone();
        let deployments = async {
            Api::<V1Deployment>::namespaced(client_clone, &namespace)
                .list(&list_param)
                .await
        };
        let pods = async {
            Api::<V1Pod>::namespaced(client, &namespace)
                .list(&list_param)
                .await
        };
        let (deployments, mut pods) = futures::try_join!(deployments, pods)?;

        let mut services = Vec::with_capacity(deployments.items.len());
        for deployment in deployments.into_iter() {
            let pod = {
                let Some(spec) = deployment.spec.as_ref() else {
                    continue;
                };
                let Some(matches_labels) = spec.selector.match_labels.as_ref() else {
                    continue;
                };

                match pods.items.iter().position(|pod| {
                    pod.metadata
                        .labels
                        .as_ref()
                        .map(|labels| matches_labels.iter().all(|(k, v)| labels.get(k) == Some(v)))
                        .unwrap_or(false)
                }) {
                    Some(pod_position) => {
                        let pod = pods.items.swap_remove(pod_position);
                        Some(pod)
                    }
                    None => None,
                }
            };

            let service = match Self::create_service_from_deployment_and_pod(deployment, pod) {
                Ok(service) => service,
                Err(e) => {
                    debug!("Deployment does not provide required data: {:?}", e);
                    continue;
                }
            };

            services.push(service);
        }

        Ok(services)
    }

    async fn create_namespace_if_necessary(
        &self,
        app_name: &AppName,
    ) -> Result<(), KubernetesInfrastructureError> {
        match Api::all(self.client().await?)
            .create(
                &PostParams::default(),
                &namespace_payload(app_name, &self.config),
            )
            .await
        {
            Ok(result) => {
                debug!(
                    "Successfully created namespace {}",
                    result
                        .metadata
                        .name
                        .unwrap_or_else(|| String::from("<unknown>"))
                );
                Ok(())
            }
            Err(KubeError::Api(ErrorResponse { code, .. })) if code == 409 => {
                debug!("Namespace {} already exists.", app_name);
                Ok(())
            }
            Err(e) => {
                error!("Cannot deploy namespace: {}", e);
                Err(e.into())
            }
        }
    }

    fn image_pull_secret<'a, I>(&self, app_name: &AppName, images: I) -> Option<V1Secret>
    where
        I: Iterator<Item = &'a Image>,
    {
        let registries_and_credentials: BTreeMap<String, (&str, &SecUtf8)> = images
            .filter_map(|image| {
                image.registry().and_then(|registry| {
                    self.config
                        .registry_credentials(&registry)
                        .map(|(username, password)| (registry, (username, password)))
                })
            })
            .collect();

        if registries_and_credentials.is_empty() {
            return None;
        }

        Some(image_pull_secret_payload(
            app_name,
            registries_and_credentials,
        ))
    }

    async fn create_payloads(
        &self,
        app_name: &AppName,
        deployabel_service: &DeployableService,
        container_config: &ContainerConfig,
    ) -> Result<
        (
            Option<V1Secret>,
            V1Service,
            V1Deployment,
            IngressRoute,
            Vec<Middleware>,
        ),
        KubernetesInfrastructureError,
    > {
        let secret = deployabel_service
            .files()
            .map(|files| secrets_payload(app_name, deployabel_service, files));

        let service = service_payload(app_name, deployabel_service);

        let deployment = deployment_payload(
            app_name,
            deployabel_service,
            container_config,
            &self
                .create_persistent_volume_claim(app_name, deployabel_service)
                .await?,
        );

        let ingress_route = ingress_route_payload(app_name, deployabel_service);
        let middlewares = middleware_payload(app_name, deployabel_service.ingress_route());

        Ok((secret, service, deployment, ingress_route, middlewares))
    }

    async fn create_persistent_volume_claim<'a>(
        &self,
        app_name: &AppName,
        service: &'a DeployableService,
    ) -> Result<Option<HashMap<&'a String, PersistentVolumeClaim>>, KubernetesInfrastructureError>
    {
        let client = self.client().await?;
        let Runtime::Kubernetes(k8s_config) = self.config.runtime_config() else {
            return Ok(None);
        };

        let storage_size = k8s_config.storage_config().storage_size();
        let storage_class = match k8s_config.storage_config().storage_class() {
            Some(sc) => sc.into(),
            None => self
                .fetch_default_storage_class()
                .await?
                .metadata
                .name
                .ok_or(KubernetesInfrastructureError::UnexpectedError {
                    internal_message: String::from(
                        "The default storage class contains an empty name",
                    ),
                })?,
        };

        let mut persistent_volume_map = HashMap::new();
        let existing_pvc: Api<PersistentVolumeClaim> =
            Api::namespaced(client.clone(), &app_name.to_rfc1123_namespace_id());

        for declared_volume in service.declared_volumes() {
            let pvc_list_params = ListParams {
                label_selector: Some(format!(
                    "{}={},{}={},{}={}",
                    APP_NAME_LABEL,
                    app_name,
                    SERVICE_NAME_LABEL,
                    service.service_name(),
                    STORAGE_TYPE_LABEL,
                    declared_volume.split('/').last().unwrap_or("default")
                )),
                ..Default::default()
            };

            let fetched_pvc = existing_pvc.list(&pvc_list_params).await?.items;

            if fetched_pvc.is_empty() {
                match Api::namespaced(client.clone(), &app_name.to_rfc1123_namespace_id())
                    .create(
                        &PostParams::default(),
                        &persistent_volume_claim_payload(
                            app_name,
                            service,
                            storage_size,
                            &storage_class,
                            declared_volume,
                        ),
                    )
                    .await
                {
                    Ok(pvc) => {
                        persistent_volume_map.insert(declared_volume, pvc);
                    }
                    Err(e) => {
                        error!("Cannot deploy persistent volume claim: {}", e);
                        return Err(e.into());
                    }
                }
            } else {
                if fetched_pvc.len() != 1 {
                    warn!(
                        "Found more than 1 Persistent Volume Claim - {:?} for declared image path {} \n Using the first available Persistent Volume Claim - {:?}",
                        &fetched_pvc.iter().map(|pvc| &pvc.metadata.name),
                        declared_volume,
                        fetched_pvc.first().unwrap().metadata.name
                    );
                }

                persistent_volume_map
                    .insert(declared_volume, fetched_pvc.into_iter().next().unwrap());
            }
        }
        Ok(Some(persistent_volume_map))
    }

    async fn fetch_default_storage_class(
        &self,
    ) -> Result<StorageClass, KubernetesInfrastructureError> {
        let storage_classes: Api<StorageClass> = Api::all(self.client().await?);

        match storage_classes.list(&ListParams::default()).await {
            Ok(sc) => sc
                .items
                .into_iter()
                .find(|sc| {
                    sc.metadata.annotations.as_ref().map_or_else(
                        || false,
                        |v| {
                            v.get("storageclass.kubernetes.io/is-default-class")
                                == Some(&"true".into())
                        },
                    )
                })
                .ok_or(KubernetesInfrastructureError::MissingDefaultStorageClass),
            Err(err) => Err(err.into()),
        }
    }
}

#[async_trait]
impl Infrastructure for KubernetesInfrastructure {
    async fn get_services(&self) -> Result<MultiMap<AppName, Service>, Error> {
        let client = self.client().await?;
        let mut app_name_and_services = Api::<V1Namespace>::all(client.clone())
            .list(&ListParams {
                label_selector: Some(APP_NAME_LABEL.to_string()),
                ..Default::default()
            })
            .await?
            .iter()
            .filter(|ns| {
                ns.status
                    .as_ref()
                    .and_then(|status| status.phase.as_ref())
                    .map(|phase| phase.as_str())
                    != Some("Terminating")
            })
            .filter_map(|ns| {
                AppName::from_str(ns.metadata.labels.as_ref()?.get(APP_NAME_LABEL)?).ok()
            })
            .map(|app_name| async {
                self.get_services_of_app(&app_name)
                    .await
                    .map(|services| (app_name, services))
            })
            .map(Box::pin)
            .collect::<FuturesUnordered<_>>();

        let mut apps = MultiMap::new();
        while let Some(res) = app_name_and_services.next().await {
            let (app_name, services) = res?;
            apps.insert_many(app_name, services);
        }

        Ok(apps)
    }

    async fn deploy_services(
        &self,
        _status_id: &str,
        deployment_unit: &DeploymentUnit,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
        let app_name = deployment_unit.app_name();
        self.create_namespace_if_necessary(app_name).await?;

        let client = self.client().await?;

        let bootstrap_image_pull_secret = self.image_pull_secret(
            app_name,
            self.config
                .companion_bootstrapping_containers()
                .iter()
                .map(|bc| bc.image()),
        );
        let mut k8s_deployment_unit = K8sDeploymentUnit::bootstrap(
            deployment_unit,
            client.clone(),
            self.config.companion_bootstrapping_containers(),
            bootstrap_image_pull_secret,
        )
        .await?;

        let services = self.get_services_of_app(app_name).await?;
        k8s_deployment_unit.filter_by_instances_and_replicas(&services);

        for deployable_service in deployment_unit.services() {
            let (secret, service, deployment, ingress_route, middlewares) = self
                .create_payloads(app_name, deployable_service, container_config)
                .await?;

            k8s_deployment_unit.merge(secret, service, deployment, ingress_route, middlewares);
        }

        if let Some(image_pull_secret) =
            self.image_pull_secret(app_name, k8s_deployment_unit.images().iter())
        {
            k8s_deployment_unit.apply_image_pull_secret(image_pull_secret);
        }

        let deployments = k8s_deployment_unit.deploy(client, app_name).await?;
        let mut services = Vec::with_capacity(deployments.len());
        for deployment in deployments.into_iter() {
            if let Ok(service) = Self::create_service_from_deployment_and_pod(deployment, None) {
                services.push(service);
            }
        }

        Ok(services)
    }

    async fn stop_services(
        &self,
        _status_id: &str,
        app_name: &AppName,
    ) -> Result<Vec<Service>, Error> {
        let services = self.get_services_of_app(app_name).await?;
        if services.is_empty() {
            return Ok(services);
        }

        Api::<V1Namespace>::all(self.client().await?)
            .delete(
                &app_name.to_rfc1123_namespace_id(),
                &DeleteParams::default(),
            )
            .await?;

        Ok(services)
    }

    async fn get_logs(
        &self,
        app_name: &AppName,
        service_name: &str,
        from: &Option<DateTime<FixedOffset>>,
        limit: usize,
    ) -> Result<Option<Vec<(DateTime<FixedOffset>, String)>>, Error> {
        let client = self.client().await?;
        let namespace = app_name.to_rfc1123_namespace_id();

        let Some((_deployment, Some(pod))) =
            self.get_deployment_and_pod(app_name, service_name).await?
        else {
            return Ok(None);
        };

        let p = LogParams {
            timestamps: true,
            since_seconds: from
                .map(|from| {
                    from.timestamp()
                        - pod
                            .status
                            .as_ref()
                            .unwrap()
                            .start_time
                            .as_ref()
                            .unwrap()
                            .0
                            .timestamp()
                })
                .filter(|since_seconds| since_seconds > &0),
            ..Default::default()
        };

        let logs = Api::<V1Pod>::namespaced(client, &namespace)
            .logs(&pod.metadata.name.unwrap(), &p)
            .await?;

        let logs = logs
            .split('\n')
            .enumerate()
            // Unfortunately,  API does not support head (also like docker, cf. https://github.com/moby/moby/issues/13096)
            // Until then we have to skip these log messages which is super slow…
            .filter(move |(index, _)| index < &limit)
            .filter(|(_, line)| !line.is_empty())
            .map(|(_, line)| {
                let mut iter = line.splitn(2, ' ');
                let timestamp = iter.next().expect(
                    "This should never happen: kubernetes should return timestamps, separated by space",
                );

                let datetime =
                    DateTime::parse_from_rfc3339(timestamp).expect("Expecting a valid timestamp");

                let mut log_line: String = iter.collect::<Vec<&str>>().join(" ");
                log_line.push('\n');
                (datetime, log_line)
            })
            .collect();

        Ok(Some(logs))
    }

    async fn change_status(
        &self,
        app_name: &AppName,
        service_name: &str,
        status: ServiceStatus,
    ) -> Result<Option<Service>, Error> {
        let Some((mut deployment, pod)) =
            self.get_deployment_and_pod(app_name, service_name).await?
        else {
            return Ok(None);
        };

        let service = Self::create_service_from_deployment_and_pod(deployment.clone(), pod)?;
        if service.status() == &status {
            return Ok(None);
        }

        let Some(spec) = deployment.spec.as_mut() else {
            return Ok(None);
        };

        spec.replicas = Some(match status {
            ServiceStatus::Running => 1,
            ServiceStatus::Paused => 0,
        });

        Api::<V1Deployment>::namespaced(self.client().await?, &app_name.to_rfc1123_namespace_id())
            .patch(
                &deployment.metadata.name.clone().unwrap(),
                &PatchParams::default(),
                &Patch::Merge(deployment),
            )
            .await?;

        Ok(Some(service))
    }

    async fn base_traefik_ingress_route(&self) -> Result<Option<TraefikIngressRoute>, Error> {
        let Runtime::Kubernetes(k8s_config) = self.config.runtime_config() else {
            return Ok(None);
        };

        let labels_path = k8s_config.downward_api().labels_path();
        let labels = match tokio::fs::read_to_string(labels_path).await {
            Ok(lables) => lables,
            Err(err) => {
                warn!(
                    "Cannot read pod labels form “{}”: {}",
                    labels_path.to_string_lossy(),
                    err
                );
                return Ok(None);
            }
        };

        let labels = labels
            .lines()
            .filter_map(|line| {
                let mut s = line.split('=');
                match (s.next(), s.next()) {
                    (Some(k), Some(v)) => Some((k.to_string(), v.trim_matches('"').to_string())),
                    _ => None,
                }
            })
            .collect::<BTreeMap<_, _>>();

        let client = self.client().await?;
        let services = Api::<V1Service>::all(client.clone())
            .list(&Default::default())
            .await?;

        let Some(service) = services.into_iter().find(|s| {
            let Some(spec) = &s.spec else { return false };
            let Some(selector) = &spec.selector else {
                return false;
            };

            if selector.is_empty() {
                return false;
            }

            for (k, v) in selector {
                match labels.get(k) {
                    Some(value) if value != v => return false,
                    None => return false,
                    Some(_) => {}
                }
            }

            true
        }) else {
            return Ok(None);
        };

        let routes = Api::<IngressRoute>::namespaced(client, &service.metadata.namespace.unwrap())
            .list(&Default::default())
            .await?;

        for r in routes {
            if let Some(routes) = &r.spec.routes {
                for route in routes {
                    for s in &route.services {
                        if let Some(name) = &service.metadata.name {
                            if &s.name == name {
                                return Ok(TraefikIngressRoute::try_from(r).ok());
                            }
                        }
                    }
                }
            }
        }

        Ok(None)
    }
}

impl TryFrom<V1Deployment> for ServiceBuilder {
    type Error = KubernetesInfrastructureError;

    fn try_from(deployment: V1Deployment) -> Result<Self, Self::Error> {
        let name = deployment.metadata.name.as_ref().ok_or(
            KubernetesInfrastructureError::UnexpectedError {
                internal_message: String::from("Missing deployment name"),
            },
        )?;
        let mut builder = ServiceBuilder::new()
            .id(name.clone())
            .config(ServiceConfig::try_from(&deployment)?);

        let labels = deployment.metadata.labels;
        builder = match labels.as_ref().and_then(|l| l.get(APP_NAME_LABEL)) {
            Some(app_name) => builder.app_name(app_name.clone()),
            None => {
                return Err(KubernetesInfrastructureError::MissingAppNameLabel {
                    deployment_name: name.to_string(),
                });
            }
        };

        builder = builder.service_status(
            deployment
                .spec
                .as_ref()
                .map(|spec| match spec.replicas {
                    None => ServiceStatus::Paused,
                    Some(replicas) if replicas <= 0 => ServiceStatus::Paused,
                    _ => ServiceStatus::Running,
                })
                .unwrap_or(ServiceStatus::Paused),
        );

        Ok(builder)
    }
}

impl TryFrom<&V1Deployment> for ServiceConfig {
    type Error = KubernetesInfrastructureError;

    fn try_from(deployment: &V1Deployment) -> Result<Self, Self::Error> {
        let deployment_name = deployment.metadata.name.as_ref().ok_or_else(|| {
            KubernetesInfrastructureError::UnexpectedError {
                internal_message: String::from("Missing deployment name"),
            }
        })?;

        if let (Some(labels), Some(annotations)) = (
            &deployment.metadata.labels,
            &deployment.metadata.annotations,
        ) {
            let service_name = labels.get(SERVICE_NAME_LABEL).unwrap_or(deployment_name);

            let image = match annotations
                .get(IMAGE_LABEL)
                .and_then(|image| Image::from_str(image).ok())
            {
                Some(img) => img,
                None => deployment
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.template.spec.as_ref())
                    .and_then(|pod_spec| pod_spec.containers.first())
                    .and_then(|container| container.image.as_ref())
                    .and_then(|image| Image::from_str(image).ok())
                    .ok_or_else(|| KubernetesInfrastructureError::MissingImageLabel {
                        deployment_name: deployment_name.clone(),
                    })?,
            };

            let mut config = ServiceConfig::new(service_name.clone(), image);

            if let Some(replicated_env) = annotations.get(REPLICATED_ENV_LABEL) {
                let env = serde_json::from_str::<Environment>(replicated_env).map_err(|err| {
                    KubernetesInfrastructureError::UnexpectedError {
                        internal_message: err.to_string(),
                    }
                })?;
                config.set_env(Some(env));
            }

            if let Some(lb) = labels.get(CONTAINER_TYPE_LABEL) {
                config.set_container_type(lb.parse::<ContainerType>()?);
            }

            Ok(config)
        } else {
            Err(KubernetesInfrastructureError::UnexpectedError {
                internal_message: String::from("Missing deployment labels, annotations, or both"),
            })
        }
    }
}

impl From<KubeError> for KubernetesInfrastructureError {
    fn from(err: KubeError) -> Self {
        KubernetesInfrastructureError::UnexpectedError {
            internal_message: err.to_string(),
        }
    }
}

impl From<ServiceError> for KubernetesInfrastructureError {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::InvalidServiceType { label } => {
                KubernetesInfrastructureError::UnknownServiceType {
                    unknown_label: label,
                }
            }
            err => KubernetesInfrastructureError::UnexpectedError {
                internal_message: err.to_string(),
            },
        }
    }
}

impl From<ServiceBuilderError> for KubernetesInfrastructureError {
    fn from(err: ServiceBuilderError) -> Self {
        KubernetesInfrastructureError::UnexpectedError {
            internal_message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EnvironmentVariable;
    use k8s_openapi::api::apps::v1::DeploymentSpec;
    use kube::api::ObjectMeta;

    macro_rules! deployment_object {
        ($deployment_name:expr, $app_name:expr, $service_name:expr, $image:expr, $container_type:expr, $($a_key:expr => $a_value:expr),*) => {{
            let mut labels = BTreeMap::new();

            if let Some(app_name) = $app_name {
                labels.insert(String::from(APP_NAME_LABEL), app_name);
            }
            if let Some(service_name) = $service_name {
                labels.insert(String::from(SERVICE_NAME_LABEL), service_name);
            }
            if let Some(container_type) = $container_type {
                labels.insert(String::from(CONTAINER_TYPE_LABEL), container_type);
            }

            let mut annotations = BTreeMap::new();
            if let Some(image) = $image {
                annotations.insert(String::from(IMAGE_LABEL), image);
            }

            $( annotations.insert(String::from($a_key), $a_value); )*

            V1Deployment {
                metadata: ObjectMeta {
                    name: Some(String::from($deployment_name)),
                    labels: Some(labels),
                    annotations: Some(annotations),
                    ..Default::default()
                },
                spec: Some(DeploymentSpec::default()),
                ..Default::default()
            }
        }};
    }

    #[test]
    fn should_parse_service_from_deployment_spec() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            None,
        );

        let service = ServiceBuilder::try_from(deployment)
            .unwrap()
            .started_at(Utc::now())
            .build()
            .unwrap();

        assert_eq!(service.app_name(), &String::from("master"));
        assert_eq!(service.service_name(), &String::from("nginx"));
    }

    #[test]
    fn should_parse_service_from_deployment_spec_with_replicated_env() {
        let deployment = deployment_object!(
            "master-db",
            Some(String::from("master")),
            Some(String::from("db")),
            Some(String::from("mariadb")),
            None,
            REPLICATED_ENV_LABEL => serde_json::json!({ "MYSQL_ROOT_PASSWORD": { "value": "example" } }).to_string()
        );

        let service = ServiceBuilder::try_from(deployment)
            .unwrap()
            .started_at(Utc::now())
            .build()
            .unwrap();

        assert_eq!(
            service.config().env().unwrap().get(0).unwrap(),
            &EnvironmentVariable::with_replicated(
                String::from("MYSQL_ROOT_PASSWORD"),
                SecUtf8::from("example")
            )
        );
    }

    #[test]
    fn should_parse_service_from_deployment_spec_without_container_type() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            None,
        );

        let service = ServiceBuilder::try_from(deployment)
            .unwrap()
            .started_at(Utc::now())
            .build()
            .unwrap();

        assert_eq!(service.container_type(), &ContainerType::Instance);
    }

    #[test]
    fn should_parse_service_from_deployment_spec_with_container_type() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            Some(String::from("replica")),
        );

        let service = ServiceBuilder::try_from(deployment)
            .unwrap()
            .started_at(Utc::now())
            .build()
            .unwrap();

        assert_eq!(service.container_type(), &ContainerType::Replica);
    }

    #[test]
    fn should_not_parse_service_from_deployment_spec_missing_app_name_label() {
        let deployment = deployment_object!(
            "master-nginx",
            None,
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            None,
        );

        let err = ServiceBuilder::try_from(deployment).unwrap_err();
        assert_eq!(
            err,
            KubernetesInfrastructureError::MissingAppNameLabel {
                deployment_name: "master-nginx".to_string()
            }
        );
    }

    #[test]
    fn should_parse_service_from_deployment_spec_with_missing_service_name_label() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            None,
            Some(String::from("nginx")),
            None,
        );

        let service = ServiceBuilder::try_from(deployment)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(service.service_name(), "master-nginx");
    }

    #[test]
    fn should_not_parse_service_from_deployment_spec_invalid_container_type() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            Some(String::from("abc")),
        );

        let err = ServiceBuilder::try_from(deployment).unwrap_err();
        assert_eq!(
            err,
            KubernetesInfrastructureError::UnknownServiceType {
                unknown_label: "abc".to_string()
            }
        );
    }

    #[test]
    fn should_not_parse_service_from_deployment_spec_due_to_missing_image_name() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            None,
            None,
        );

        let err = ServiceBuilder::try_from(deployment).unwrap_err();
        assert_eq!(
            err,
            KubernetesInfrastructureError::MissingImageLabel {
                deployment_name: "master-nginx".to_string()
            }
        );
    }
}
