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
use crate::infrastructure::kubernetes::payloads::namespace_annotations;
use crate::infrastructure::traefik::{TraefikIngressRoute, TraefikMiddleware};
use crate::infrastructure::{
    HttpForwarder, Infrastructure, TraefikRouterRule, OWNERS_LABEL, USER_DEFINED_PARAMETERS_LABEL,
};
use crate::models::user_defined_parameters::UserDefinedParameters;
use crate::models::{
    App, AppName, ContainerType, Environment, Image, Owner, Service, ServiceConfig, ServiceError,
    ServiceStatus, State, WebHostMeta,
};
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use futures::stream::FuturesUnordered;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use futures::{AsyncBufReadExt, TryStreamExt};
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use k8s_openapi::api::storage::v1::StorageClass;
use k8s_openapi::api::{
    apps::v1::Deployment as V1Deployment, core::v1::Namespace as V1Namespace,
    core::v1::Pod as V1Pod, core::v1::Secret as V1Secret, core::v1::Service as V1Service,
};
use kube::api::ObjectMeta;
use kube::{
    api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams},
    client::Client,
    config::Config,
    error::{Error as KubeError, ErrorResponse},
};
use kube::{Resource, ResourceExt};
use log::{debug, error, warn};
use secstr::SecUtf8;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::{From, TryFrom};
use std::str::FromStr;

#[derive(Clone)]
pub struct KubernetesInfrastructure {
    config: PREvantConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum KubernetesInfrastructureError {
    #[error("Unexpected Kubernetes interaction error: {err}")]
    UnexpectedError { err: anyhow::Error },
    #[error("Unknown service type label {unknown_label}")]
    UnknownServiceType { unknown_label: String },
    #[error("The deployment {deployment_name} does not provide a label for image.")]
    MissingImageLabel { deployment_name: String },
    #[error("The default storage class is missing in kubernetes.")]
    MissingDefaultStorageClass,
    #[error("The default storage class contains an empty name")]
    DefaultStorageClassWithoutName,
    #[error("Missing deployment name")]
    DeploymentWithoutName,
    #[error("Missing deployment labels, annotations, or both")]
    MissingDeploymentAnnotations,
    #[error("Bootstrap pod {pod_name} for {app_name} failed")]
    BootstrapContainerFailed { pod_name: String, app_name: AppName },
}

impl KubernetesInfrastructure {
    pub fn new(config: PREvantConfig) -> Self {
        Self { config }
    }

    async fn client(&self) -> Result<Client, KubernetesInfrastructureError> {
        let configuration = Config::infer().await.map_err(|err| {
            KubernetesInfrastructureError::UnexpectedError {
                err: anyhow::Error::new(err)
                    .context("Failed to read Kube configuration from cluster env"),
            }
        })?;

        Client::try_from(configuration).map_err(|err| {
            KubernetesInfrastructureError::UnexpectedError {
                err: anyhow::Error::new(err).context("Failed to create client"),
            }
        })
    }

    async fn get_deployment_and_pod(
        &self,
        app_name: &AppName,
        service_name: &str,
    ) -> Result<Option<(V1Deployment, Option<V1Pod>)>, KubernetesInfrastructureError> {
        let client = self.client().await?;
        Self::get_deployment_and_pod_impl(client, app_name, service_name).await
    }

    async fn get_deployment_and_pod_impl(
        client: kube::Client,
        app_name: &AppName,
        service_name: &str,
    ) -> Result<Option<(V1Deployment, Option<V1Pod>)>, KubernetesInfrastructureError> {
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

    async fn create_namespace_if_necessary(
        &self,
        deployment_unit: &DeploymentUnit,
    ) -> Result<(), KubernetesInfrastructureError> {
        let app_name = deployment_unit.app_name();

        let api = Api::all(self.client().await?);
        match api
            .create(
                &PostParams::default(),
                &namespace_payload(
                    app_name,
                    &self.config,
                    deployment_unit.user_defined_parameters(),
                    deployment_unit.owners(),
                ),
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
            Err(KubeError::Api(ErrorResponse { code: 409, .. })) => {
                debug!("Namespace {app_name} already exists.");

                let annotations = namespace_annotations(
                    &self.config,
                    deployment_unit.user_defined_parameters(),
                    deployment_unit.owners(),
                );
                if annotations.is_some() {
                    debug!("Patching namespace {app_name} with user defined parameters.");
                    api.patch(
                        &app_name.to_rfc1123_namespace_id(),
                        &PatchParams::apply("PREvant"),
                        &Patch::Merge(&V1Namespace {
                            metadata: ObjectMeta {
                                annotations,
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                    )
                    .await?;
                }

                Ok(())
            }
            Err(e) => {
                error!("Cannot deploy namespace: {e}");
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
        deployable_service: &DeployableService,
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
        let secret = deployable_service
            .files()
            .map(|files| secrets_payload(app_name, deployable_service, files));

        let service = service_payload(app_name, deployable_service);

        let deployment = deployment_payload(
            app_name,
            deployable_service,
            container_config,
            &self
                .create_persistent_volume_claim(app_name, deployable_service)
                .await?,
        );

        let ingress_route = ingress_route_payload(app_name, deployable_service);
        let middlewares = middleware_payload(app_name, deployable_service.ingress_route());

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
                .ok_or(KubernetesInfrastructureError::DefaultStorageClassWithoutName)?,
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
                    declared_volume.split('/').next_back().unwrap_or("default")
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
                        error!("Cannot deploy persistent volume claim: {e}");
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

    fn parse_user_defined_parameters_from(
        &self,
        namespace: &V1Namespace,
    ) -> Option<UserDefinedParameters> {
        let validator = self.config.user_defined_schema_validator()?;

        let udp = namespace
            .metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(USER_DEFINED_PARAMETERS_LABEL))?;

        let data = serde_json::from_str(udp)
            .inspect_err(|e| {
                warn!(
                    "Cannot parse user defined parameters {}: {e}",
                    namespace.metadata.name.as_deref().unwrap_or_default()
                )
            })
            .ok()?;

        UserDefinedParameters::new(data, &validator)
            .inspect_err(|e| {
                warn!(
                    "Cannot validate user defined parameters {}: {e}",
                    namespace.metadata.name.as_deref().unwrap_or_default()
                )
            })
            .ok()
    }
}

#[async_trait]
impl Infrastructure for KubernetesInfrastructure {
    async fn fetch_apps(&self) -> Result<HashMap<AppName, App>> {
        let mut app_name_and_services = self
            .fetch_app_names()
            .await?
            .into_iter()
            .map(|app_name| async {
                self.fetch_app(&app_name)
                    .await
                    .map(|services| (app_name, services))
            })
            .map(Box::pin)
            .collect::<FuturesUnordered<_>>();

        let mut apps = HashMap::new();
        while let Some(res) = app_name_and_services.next().await {
            let (app_name, services) = res?;
            if let Some(services) = services {
                apps.insert(app_name, services);
            }
        }

        Ok(apps)
    }

    async fn fetch_app(&self, app_name: &AppName) -> Result<Option<App>> {
        let namespace = app_name.to_rfc1123_namespace_id();
        let list_param = Default::default();

        let pods_client = self.client().await?;
        let deployments_client = pods_client.clone();
        let namespace_client = pods_client.clone();
        let deployments = async {
            Api::<V1Deployment>::namespaced(deployments_client, &namespace)
                .list(&list_param)
                .await
        };
        let pods = async {
            Api::<V1Pod>::namespaced(pods_client, &namespace)
                .list(&list_param)
                .await
        };
        let namespace = async {
            Api::<V1Namespace>::all(namespace_client)
                .get_opt(&namespace)
                .await
        };
        let (deployments, mut pods, namespace) = futures::try_join!(deployments, pods, namespace)?;

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

            let service = match Service::try_from((deployment, pod)) {
                Ok(service) => service,
                Err(e) => {
                    debug!("Deployment does not provide required data: {e:?}");
                    continue;
                }
            };

            services.push(service);
        }

        if services.is_empty() {
            return Ok(None);
        }

        let udp = namespace
            .as_ref()
            .and_then(|namespace| self.parse_user_defined_parameters_from(namespace));

        let owners = namespace
            .and_then(|mut namespace| namespace.annotations_mut().remove(OWNERS_LABEL))
            .and_then(|owners_payload| serde_json::from_str::<HashSet<Owner>>(&owners_payload).ok())
            .unwrap_or_else(HashSet::new);

        Ok(Some(App::new(services, owners, udp)))
    }

    async fn fetch_app_names(&self) -> Result<HashSet<AppName>> {
        let client = self.client().await?;
        Ok(Api::<V1Namespace>::all(client)
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
            .collect::<HashSet<_>>())
    }

    async fn deploy_services(
        &self,
        deployment_unit: &DeploymentUnit,
        container_config: &ContainerConfig,
    ) -> Result<App> {
        self.create_namespace_if_necessary(deployment_unit).await?;

        let client = self.client().await?;

        let app_name = deployment_unit.app_name();
        let bootstrapping_containers = self.config.companion_bootstrapping_containers(
            app_name,
            &deployment_unit.app_base_route().to_url(),
            Some(serde_json::json!({
                "namespace": app_name.to_rfc1123_namespace_id()
            })),
            deployment_unit.user_defined_parameters(),
        )?;

        let bootstrap_image_pull_secret = self.image_pull_secret(
            app_name,
            bootstrapping_containers.iter().map(|bc| &bc.image),
        );
        let mut k8s_deployment_unit = K8sDeploymentUnit::bootstrap(
            deployment_unit,
            client.clone(),
            &bootstrapping_containers,
            bootstrap_image_pull_secret,
        )
        .await?;

        let deployment_unit_service_names = deployment_unit
            .services()
            .iter()
            .map(|s| s.service_name())
            .collect::<HashSet<_>>();

        if let Some(app) = self.fetch_app(app_name).await? {
            k8s_deployment_unit.filter_by_instances_and_replicas(
                app.services()
                    .iter()
                    // We must exclude the services that are provided by the deployment_unit
                    // because without that filter a second update of the service would create an
                    // additional Kubernetes deployment instead of updating/merging the existing one
                    // that had been created by the bootstrap containers.
                    .filter(|s| !deployment_unit_service_names.contains(s.service_name())),
            );
        }

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
            if let Ok(service) = Service::try_from((deployment, None)) {
                services.push(service);
            }
        }

        Ok(App::new(
            services,
            deployment_unit.owners().clone(),
            deployment_unit.user_defined_parameters().clone(),
        ))
    }

    async fn stop_services(&self, app_name: &AppName) -> Result<App> {
        let Some(services) = self.fetch_app(app_name).await? else {
            return Ok(App::empty());
        };

        Api::<V1Namespace>::all(self.client().await?)
            .delete(
                &app_name.to_rfc1123_namespace_id(),
                &DeleteParams::default(),
            )
            .await?;

        Ok(services)
    }

    async fn get_logs<'a>(
        &'a self,
        app_name: &'a AppName,
        service_name: &'a str,
        from: &'a Option<DateTime<FixedOffset>>,
        limit: &'a Option<usize>,
        follow: bool,
    ) -> BoxStream<'a, Result<(DateTime<FixedOffset>, String)>> {
        let Some((_deployment, Some(pod))) =
            (match self.get_deployment_and_pod(app_name, service_name).await {
                Ok(result) => result,
                Err(_) => return stream::empty().boxed(),
            })
        else {
            return stream::empty().boxed();
        };

        stream! {
            let p = LogParams {
                timestamps: true,
                since_time: from.map(|from| from.with_timezone(&Utc)),
                follow,
                ..Default::default()
            };
            let client = self.client().await?;
            let namespace = app_name.to_rfc1123_namespace_id();

            let logs = Api::<V1Pod>::namespaced(client, &namespace)
                .log_stream(&pod.metadata.name.unwrap(), &p)
                .await?;
            let mut logs = match limit {
                Some(log_limit) => {
                    Box::pin(logs.lines().take(*log_limit)) as BoxStream<Result<String, std::io::Error>>
                }
                None => Box::pin(logs.lines()) as BoxStream<Result<String, std::io::Error>>,
            };
            while let Some(line) = logs.try_next().await? {
                let mut iter = line.splitn(2, ' ');
                let timestamp = iter.next().expect(
                    "This should never happen: kubernetes should return timestamps, separated by space",
                );

                let datetime =
                    DateTime::parse_from_rfc3339(timestamp).expect("Expecting a valid timestamp");

                let mut log_line: String = iter.collect::<Vec<&str>>().join(" ");
                log_line.push('\n');

                yield Ok((datetime, log_line))
            }
        }.boxed()
    }

    async fn change_status(
        &self,
        app_name: &AppName,
        service_name: &str,
        status: ServiceStatus,
    ) -> Result<Option<Service>> {
        let Some((mut deployment, pod)) =
            self.get_deployment_and_pod(app_name, service_name).await?
        else {
            return Ok(None);
        };

        let service = Service::try_from((deployment.clone(), pod))?;
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

    async fn http_forwarder(&self) -> Result<Box<dyn HttpForwarder>> {
        let client = self.client().await?;
        Ok(Box::new(K8sHttpForwarder { client }))
    }

    async fn base_traefik_ingress_route(&self) -> Result<Option<TraefikIngressRoute>> {
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
        let api = Api::<V1Service>::all(client);
        let services = api.list(&Default::default()).await?;

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

        let api = Api::<IngressRoute>::namespaced(
            api.into_client(),
            &service.metadata.namespace.clone().unwrap(),
        );
        let routes = api.list(&Default::default()).await?;

        let Some((ingress_route, inner_route)) = routes
            .iter()
            .filter_map(|r| Some((r, r.spec.routes.as_ref()?)))
            .filter_map(|(ingress_route, routes)| {
                for route in routes {
                    for s in &route.services {
                        if Some(&s.name) == service.meta().name.as_ref() {
                            return Some((ingress_route, route.clone()));
                        }
                    }
                }

                None
            })
            .next()
        else {
            return Ok(None);
        };

        let api = Api::<Middleware>::namespaced(
            api.into_client(),
            &service.metadata.namespace.clone().unwrap(),
        );
        let mut middlewares = inner_route
            .middlewares
            .iter()
            .flatten()
            .map(|m| api.get(&m.name))
            .collect::<FuturesUnordered<_>>();

        let mut traefik_middlewares = Vec::with_capacity(middlewares.len());
        while let Some(middleware) = middlewares.try_next().await? {
            let middleware = TraefikMiddleware {
                name: middleware.metadata.name.expect("There should be a name"),
                spec: serde_value::to_value(middleware.spec.0).expect("should be convertible"),
            };

            traefik_middlewares.push(middleware);
        }

        Ok(Some(TraefikIngressRoute::with_existing_routing_rules(
            ingress_route.spec.entry_points.clone().unwrap_or_default(),
            TraefikRouterRule::from_str(&inner_route.r#match).unwrap(),
            traefik_middlewares,
            ingress_route
                .spec
                .tls
                .clone()
                .unwrap_or_default()
                .cert_resolver,
        )))
    }
}

#[derive(Clone)]
struct K8sHttpForwarder {
    client: kube::Client,
}

#[async_trait]
impl HttpForwarder for K8sHttpForwarder {
    async fn request_web_host_meta(
        &self,
        app_name: &AppName,
        service_name: &str,
        request: http::Request<Empty<bytes::Bytes>>,
    ) -> Result<Option<WebHostMeta>>
    where
        Self: Sized,
    {
        let Some((_deployment, Some(pod))) = KubernetesInfrastructure::get_deployment_and_pod_impl(
            self.client.clone(),
            app_name,
            service_name,
        )
        .await?
        else {
            return Ok(None);
        };

        let port = pod
            .spec
            .as_ref()
            .and_then(|spec| spec.containers.first())
            .and_then(|container| {
                container
                    .ports
                    .as_ref()
                    .and_then(|ports| ports.first())
                    .map(|port| port.container_port as u16)
            })
            .unwrap_or(80u16);

        let client = self.client.clone();

        let pods = Api::<V1Pod>::namespaced(client, &app_name.to_rfc1123_namespace_id());
        let mut pf = pods
            .portforward(pod.metadata.name.as_ref().unwrap(), &[port])
            .await?;
        let port = pf.take_stream(port).unwrap();

        // let hyper drive the HTTP state in our DuplexStream via a task
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(port)).await?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                warn!("Error in connection: {e}");
            }
        });

        let (_parts, body) = sender.send_request(request).await?.into_parts();

        let body_bytes = body.collect().await?.to_bytes();

        Ok(serde_json::from_slice::<WebHostMeta>(&body_bytes).ok())
    }
}

impl TryFrom<(V1Deployment, Option<V1Pod>)> for Service {
    type Error = KubernetesInfrastructureError;

    fn try_from(deployment_and_pod: (V1Deployment, Option<V1Pod>)) -> Result<Self, Self::Error> {
        let service_config = ServiceConfig::try_from(&deployment_and_pod.0)?;

        let name = deployment_and_pod
            .0
            .metadata
            .name
            .ok_or(KubernetesInfrastructureError::DeploymentWithoutName)?;

        let status = deployment_and_pod
            .0
            .spec
            .as_ref()
            .map(|spec| match spec.replicas {
                None => ServiceStatus::Paused,
                Some(replicas) if replicas <= 0 => ServiceStatus::Paused,
                _ => ServiceStatus::Running,
            })
            .unwrap_or(ServiceStatus::Paused);

        let started_at = deployment_and_pod.1.and_then(|pod| {
            pod.status
                .as_ref()
                .and_then(|s| s.start_time.as_ref())
                .map(|t| t.0)
        });

        Ok(Service {
            id: name,
            config: service_config,
            state: State { status, started_at },
        })
    }
}

impl TryFrom<&V1Deployment> for ServiceConfig {
    type Error = KubernetesInfrastructureError;

    fn try_from(deployment: &V1Deployment) -> Result<Self, Self::Error> {
        let deployment_name = deployment
            .metadata
            .name
            .as_ref()
            .ok_or_else(|| KubernetesInfrastructureError::DeploymentWithoutName)?;

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
                        err: anyhow::Error::new(err),
                    }
                })?;
                config.set_env(Some(env));
            }

            if let Some(lb) = labels.get(CONTAINER_TYPE_LABEL) {
                config.set_container_type(lb.parse::<ContainerType>()?);
            }

            Ok(config)
        } else {
            Err(KubernetesInfrastructureError::MissingDeploymentAnnotations)
        }
    }
}

impl From<KubeError> for KubernetesInfrastructureError {
    fn from(err: KubeError) -> Self {
        KubernetesInfrastructureError::UnexpectedError {
            err: anyhow::Error::new(err),
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
                err: anyhow::Error::new(err),
            },
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
        ($deployment_name:expr_2021, $app_name:expr_2021, $service_name:expr_2021, $image:expr_2021, $container_type:expr_2021, $($a_key:expr_2021 => $a_value:expr_2021),*) => {{
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

        let service = Service::try_from((deployment, None)).unwrap();

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

        let service = Service::try_from((deployment, None)).unwrap();

        assert_eq!(
            service.config.env().unwrap().get(0).unwrap(),
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

        let service = Service::try_from((deployment, None)).unwrap();

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

        let service = Service::try_from((deployment, None)).unwrap();

        assert_eq!(service.container_type(), &ContainerType::Replica);
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

        let service = Service::try_from((deployment, None)).unwrap();
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

        let err = Service::try_from((deployment, None)).unwrap_err();
        assert!(
            matches!(err, KubernetesInfrastructureError::UnknownServiceType {
                    unknown_label
                } if unknown_label == "abc"
            )
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

        let err = Service::try_from((deployment, None)).unwrap_err();
        assert!(matches!(err,
            KubernetesInfrastructureError::MissingImageLabel {
                deployment_name
            } if deployment_name == "master-nginx"
        ));
    }
}
