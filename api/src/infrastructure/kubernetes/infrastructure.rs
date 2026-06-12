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
use super::super::{APP_NAME_LABEL, SERVICE_NAME_LABEL, STORAGE_TYPE_LABEL};
use super::deployment_unit::K8sDeploymentUnit;
use super::payloads::{
    deployment_payload, image_pull_secret_payload, ingress_route_payload, middleware_payload,
    namespace_payload, persistent_volume_claim_payload, secrets_payload, service_payload,
};
use super::traefik_crds::{IngressRoute, Middleware};
use crate::config::{Config as PREvantConfig, ContainerConfig, Runtime};
use crate::infrastructure::kubernetes::payloads::kubernetes_object_to_service;
use crate::infrastructure::{
    HttpForwarder, Infrastructure, OWNERS_LABEL, USER_DEFINED_PARAMETERS_LABEL,
    kubernetes::payloads::namespace_annotations,
};
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use domain::{
    AppName, Image, Owner, RawInfrastructureElement,
    app_blueprints::{DesiredServiceStatus, UserDefinedParameters},
    app_deployment::{
        BootstrapCompanionsWithRawElementsContext, BootstrappedCompanions, DeployableService,
        DeploymentUnit, MergeRawElementsContext,
    },
    app_instance::{App, ContainerTypeParseError, Service, ServiceStatus, WebHostMeta},
    templating::TemplateData,
    traefik::{TraefikIngressRoute, TraefikMiddleware, TraefikRouterRule},
};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use futures::stream::{self, BoxStream};
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
use kube::config::Kubeconfig;
use kube::{Resource, ResourceExt};
use kube::{
    api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams},
    client::Client,
    config::Config,
    error::{Error as KubeError, ErrorResponse},
};
use log::{debug, error, warn};
use regex::Regex;
use secstr::SecUtf8;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    convert::{From, TryFrom},
    str::FromStr,
};

#[derive(Clone)]
pub struct KubernetesInfrastructure {
    config: PREvantConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum KubernetesInfrastructureError {
    #[error("Failed to create Kubernetes client: {err}")]
    CannotInitializeClient { err: anyhow::Error },
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
    #[error("Bootstrap pod {pod_name} for {app_name} failed")]
    BootstrapContainerFailed { pod_name: String, app_name: AppName },
}

enum NamespaceCreationResponse {
    New(V1Namespace),
    Updated(V1Namespace),
    Existed(V1Namespace),
}

impl From<NamespaceCreationResponse> for V1Namespace {
    fn from(value: NamespaceCreationResponse) -> Self {
        match value {
            NamespaceCreationResponse::New(namespace) => namespace,
            NamespaceCreationResponse::Updated(namespace) => namespace,
            NamespaceCreationResponse::Existed(namespace) => namespace,
        }
    }
}

impl KubernetesInfrastructure {
    pub fn new(config: PREvantConfig) -> Self {
        Self { config }
    }

    async fn client(&self) -> Result<Client, KubernetesInfrastructureError> {
        let configuration = match &self.config.runtime {
            Runtime::Kubernetes(k8s_config) if k8s_config.kube_config.is_some() => {
                let config_file = k8s_config.kube_config.as_ref().unwrap();
                let config = tokio::fs::read_to_string(&config_file)
                    .await
                    .map_err(
                        |err| KubernetesInfrastructureError::CannotInitializeClient {
                            err: anyhow::Error::new(err).context(format!(
                                "Failed to read Kube configuration from config file {config_file}",
                                config_file = config_file.to_string_lossy(),
                            )),
                        },
                    )?;
                Config::from_custom_kubeconfig(
                    Kubeconfig::deserialize(serde_norway::Deserializer::from_str(&config))
                        .map_err(
                            |err| KubernetesInfrastructureError::CannotInitializeClient {
                                err: anyhow::Error::new(err).context(format!(
                                    "Failed to deserialize Kube configuration from config file {config_file}",
                                    config_file = config_file.to_string_lossy(),
                                )),
                            },
                        )?,
                    &Default::default(),
                )
                .await
                .map_err(
                    |err| KubernetesInfrastructureError::CannotInitializeClient {
                        err: anyhow::Error::new(err).context(format!(
                            "Failed to initialize Kube configuration from config file {config_file}",
                            config_file = config_file.to_string_lossy(),
                        )),
                    },
                )?
            }
            _ => Config::infer().await.map_err(|err| {
                KubernetesInfrastructureError::CannotInitializeClient {
                    err: anyhow::Error::new(err)
                        .context("Failed to read Kube configuration from cluster env"),
                }
            })?,
        };

        Client::try_from(configuration).map_err(|err| {
            KubernetesInfrastructureError::CannotInitializeClient {
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
        app_name: &AppName,
        user_defined_parameters: Option<&UserDefinedParameters>,
        owners: &HashSet<Owner>,
    ) -> Result<NamespaceCreationResponse, KubernetesInfrastructureError> {
        let namespace = app_name.to_rfc1123_namespace_id();

        let api = Api::all(self.client().await?);
        match api
            .create(
                &PostParams::default(),
                &namespace_payload(app_name, &self.config, user_defined_parameters, owners),
            )
            .await
        {
            Ok(result) => {
                debug!("Successfully created namespace {namespace} for {app_name}",);
                Ok(NamespaceCreationResponse::New(result))
            }
            Err(KubeError::Api(ErrorResponse { code: 409, .. })) => {
                debug!("Namespace {app_name} already exists.");

                let annotations =
                    namespace_annotations(&self.config, user_defined_parameters, owners);
                if annotations.is_some() {
                    debug!("Patching namespace {app_name} with user defined parameters.");
                    Ok(NamespaceCreationResponse::Updated(
                        api.patch(
                            &namespace,
                            &PatchParams::apply("PREvant"),
                            &Patch::Merge(&V1Namespace {
                                metadata: ObjectMeta {
                                    annotations,
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        )
                        .await?,
                    ))
                } else {
                    Ok(NamespaceCreationResponse::Existed(
                        api.get(&namespace).await?,
                    ))
                }
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
        let secret = secrets_payload(app_name, &deployable_service.blueprint_service);

        let service = service_payload(app_name, deployable_service);

        let deployment = deployment_payload(
            app_name,
            deployable_service,
            container_config,
            &self
                .create_persistent_volume_claim(app_name, deployable_service)
                .await?,
        );

        let ingress_route = ingress_route_payload(
            app_name,
            &deployable_service.blueprint_service,
            &deployable_service.ingress_route,
            &deployable_service.service_type,
            Some(deployable_service.port),
        );
        let middlewares = middleware_payload(app_name, &deployable_service.ingress_route);

        Ok((secret, service, deployment, ingress_route, middlewares))
    }

    async fn create_persistent_volume_claim<'a>(
        &self,
        app_name: &AppName,
        service: &'a DeployableService,
    ) -> Result<Option<HashMap<&'a String, PersistentVolumeClaim>>, KubernetesInfrastructureError>
    {
        let client = self.client().await?;
        let Runtime::Kubernetes(k8s_config) = &self.config.runtime else {
            return Ok(None);
        };

        let storage_size = &k8s_config.storage_config.storage_size;
        let storage_class = match &k8s_config.storage_config.storage_class {
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

        for declared_volume in &service.declared_volumes {
            let pvc_list_params = ListParams {
                label_selector: Some(format!(
                    "{}={},{}={},{}={}",
                    APP_NAME_LABEL,
                    app_name,
                    SERVICE_NAME_LABEL,
                    service.blueprint_service.service_name,
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

        let data = serde_json::from_str::<serde_json::Value>(udp)
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
        let client = self.client().await?;
        let app_names = Api::<V1Namespace>::all(client)
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
            .collect::<HashSet<_>>();

        let mut app_name_and_services = app_names
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

            let service = match kubernetes_object_to_service(deployment, pod) {
                Ok(service) => service,
                Err(e) => {
                    debug!("Deployment does not provide required data: {e:?}");
                    continue;
                }
            };

            services.push(service);
        }

        let udp = namespace
            .as_ref()
            .and_then(|namespace| self.parse_user_defined_parameters_from(namespace));

        let created_at = namespace
            .as_ref()
            .and_then(|namespace| namespace.metadata.creation_timestamp.as_ref())
            .map(|creation_timestamp| creation_timestamp.0);

        let owners = namespace
            .and_then(|mut namespace| namespace.annotations_mut().remove(OWNERS_LABEL))
            .and_then(|owners_payload| serde_json::from_str::<HashSet<Owner>>(&owners_payload).ok())
            .unwrap_or_else(HashSet::new);

        Ok(Some(App::new(services, owners, udp, created_at)))
    }

    async fn fetch_traefik_router_names(
        &self,
        app_names: Vec<AppName>,
    ) -> Result<HashMap<AppName, Vec<Regex>>> {
        let mut client = Some(self.client().await?);

        let mut app_names_and_router_names = HashMap::new();
        for app_name in app_names {
            let api = Api::<IngressRoute>::namespaced(
                client.take().unwrap(),
                &app_name.to_rfc1123_namespace_id(),
            );

            let response = api
                .list(&ListParams {
                    // TODO: eventually PREvant should set labels to make sure to get only the ones
                    // that are managed by PREvant.
                    // label_selector: Some(format!("{APP_NAME_LABEL}={app_name}")),
                    ..Default::default()
                })
                .await?;

            let router_names = response
                .into_iter()
                .filter_map(|route| {
                    Regex::new(&format!(
                        "^{namespace}-{name}-[a-zA-Z0-9]+@kubernetescrd$",
                        namespace = route.metadata.namespace?,
                        name = route.metadata.name?
                    ))
                    .ok()
                })
                .collect::<Vec<_>>();

            if router_names.is_empty() {
                log::warn!("Cannot find router names for {app_name}");
            } else {
                app_names_and_router_names.insert(app_name, router_names);
            }

            client = Some(api.into_client());
        }

        Ok(app_names_and_router_names)
    }

    async fn fetch_app_as_backup_based_infrastructure_payload(
        &self,
        app_name: &AppName,
    ) -> Result<Option<Vec<serde_json::Value>>> {
        let client = self.client().await?;

        let unit = K8sDeploymentUnit::fetch(client, app_name).await?;
        if unit.is_empty() {
            return Ok(None);
        }

        Ok(Some(unit.prepare_for_back_up().to_json_vec()))
    }

    async fn deploy_services(
        &self,
        deployment_unit: &DeploymentUnit,
        container_config: &ContainerConfig,
    ) -> Result<App> {
        let namespace_creation_response = self
            .create_namespace_if_necessary(
                &deployment_unit.app_name,
                deployment_unit.user_defined_parameters.as_ref(),
                &deployment_unit.owners,
            )
            .await?;

        let client = self.client().await?;

        let app_name = &deployment_unit.app_name;

        // TODO: eventually this code should be refactored too because
        // BootstrapCompanions::{bootstrap_companions_with_raw_elements,
        // update_raw_elements_after_merged_blueprint_config} and K8sDeploymentUnit::merge do
        // overlapping things.
        let mut k8s_deployment_unit = K8sDeploymentUnit::parse_from_json(
            app_name,
            deployment_unit
                .bootstrapped_companion_elements
                .iter()
                .chain(
                    deployment_unit
                        .services
                        .iter()
                        .flat_map(|service| service.bootstrapped_companion_elements.iter()),
                )
                .map(|raw| raw.as_json()),
        )?;

        for deployable_service in &deployment_unit.services {
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
            if let Ok(service) = kubernetes_object_to_service(deployment, None) {
                services.push(service);
            }
        }

        let created_at = V1Namespace::from(namespace_creation_response)
            .metadata
            .creation_timestamp
            .as_ref()
            .map(|creation_timestamp| creation_timestamp.0);

        Ok(App::new(
            services,
            deployment_unit.owners.clone(),
            deployment_unit.user_defined_parameters.clone(),
            created_at,
        ))
    }

    async fn stop_services(&self, app_name: &AppName) -> Result<Option<App>> {
        let Some(app) = self.fetch_app(app_name).await? else {
            return Ok(None);
        };

        Api::<V1Namespace>::all(self.client().await?)
            .delete(
                &app_name.to_rfc1123_namespace_id(),
                &DeleteParams::default(),
            )
            .await?;

        Ok(Some(app))
    }

    async fn delete_infrastructure_objects_partially(
        &self,
        app_name: &AppName,
        infrastructure_payload: &[serde_json::Value],
    ) -> Result<()> {
        let unit = K8sDeploymentUnit::parse_from_json(app_name, infrastructure_payload)?;
        unit.prepare_for_back_up()
            .delete(self.client().await?, app_name)
            .await
    }

    async fn restore_infrastructure_objects_partially(
        &self,
        app_name: &AppName,
        infrastructure_payload: &[serde_json::Value],
    ) -> Result<Option<App>> {
        let unit = K8sDeploymentUnit::parse_from_json(app_name, infrastructure_payload)?;
        unit.deploy(self.client().await?, app_name).await?;
        self.fetch_app(app_name).await
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
        status: DesiredServiceStatus,
    ) -> Result<Option<Service>> {
        let Some((mut deployment, pod)) =
            self.get_deployment_and_pod(app_name, service_name).await?
        else {
            return Ok(None);
        };

        let service = kubernetes_object_to_service(deployment.clone(), pod)?;

        if matches!(
            (&service.status, &status),
            (ServiceStatus::Running { .. }, DesiredServiceStatus::Running)
                | (ServiceStatus::Paused, DesiredServiceStatus::Paused)
        ) {
            return Ok(None);
        }

        let Some(spec) = deployment.spec.as_mut() else {
            return Ok(None);
        };

        spec.replicas = Some(match status {
            DesiredServiceStatus::Running => 1,
            DesiredServiceStatus::Paused => 0,
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
        let Runtime::Kubernetes(k8s_config) = &self.config.runtime else {
            return Ok(None);
        };

        let labels_path = &k8s_config.downward_api.labels_path;
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
            let middleware = TraefikMiddleware::from_json(
                middleware.metadata.name.expect("There should be a name"),
                middleware.spec.0,
            )?;

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

    async fn bootstrap_companions_with_raw_elements(
        &self,
        context: BootstrapCompanionsWithRawElementsContext<'_>,
        template_data: &TemplateData,
    ) -> Result<BootstrappedCompanions> {
        let namespace_creation_response = self
            .create_namespace_if_necessary(
                context.app_name,
                context.user_defined_parameters,
                context.owners,
            )
            .await?;

        let bootstrapping_containers = self
            .config
            .companions
            .companion_bootstrapping_containers(template_data)?;

        let bootstrap_image_pull_secret = self.image_pull_secret(
            context.app_name,
            bootstrapping_containers.iter().map(|bc| &bc.image),
        );
        let client = self.client().await?;
        let k8s_deployment_unit = match K8sDeploymentUnit::bootstrap(
            client.clone(),
            context.app_name,
            &bootstrapping_containers,
            bootstrap_image_pull_secret,
        )
        .await
        {
            Ok(k8s_deployment_unit) => k8s_deployment_unit,
            Err(err) => {
                if let NamespaceCreationResponse::New(namespace) = namespace_creation_response {
                    let api = Api::<V1Namespace>::all(client);
                    if let Err(err) = api
                        .delete(
                            namespace.metadata.name.as_deref().unwrap(),
                            &DeleteParams::default(),
                        )
                        .await
                    {
                        log::error!(
                            "Cannot delete namespace after bootstrapping for {} failed: {err}",
                            context.app_name
                        );
                    }
                }
                return Err(err);
            }
        };

        BootstrappedCompanions::try_from(k8s_deployment_unit)
    }

    fn update_raw_elements_after_merged_blueprint_config(
        &self,
        context: MergeRawElementsContext<'_>,
        raw_elements: Vec<RawInfrastructureElement>,
    ) -> Result<Vec<RawInfrastructureElement>> {
        Ok(K8sDeploymentUnit::parse_from_json(
            &AppName::master(),
            raw_elements.into_iter().map(serde_json::Value::from),
        )?
        .update_with_merge_context(context)?
        .to_json_vec()
        .into_iter()
        .map(RawInfrastructureElement::from)
        .collect::<Vec<_>>())
    }

    async fn resolve_infrastructure_template_data(
        &self,
        app_name: &AppName,
    ) -> Result<Option<serde_json::Value>> {
        Ok(Some(serde_json::json!({
            "namespace": app_name.to_rfc1123_namespace_id()
        })))
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

impl From<KubeError> for KubernetesInfrastructureError {
    fn from(err: KubeError) -> Self {
        KubernetesInfrastructureError::UnexpectedError {
            err: anyhow::Error::new(err),
        }
    }
}

impl From<ContainerTypeParseError> for KubernetesInfrastructureError {
    fn from(err: ContainerTypeParseError) -> Self {
        match err {
            ContainerTypeParseError::Unknow { label } => {
                KubernetesInfrastructureError::UnknownServiceType {
                    unknown_label: label,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{apps::AppsError, config::runtime::KubernetesRuntimeConfig};
    use domain::{
        RawInfrastructureElement,
        app_deployment::{AppDeploymentBuilder, MergeRawElementsContext},
        app_instance::ContainerType,
        blueprint_service,
    };
    use std::convert::Infallible;
    use tempfile::TempDir;
    use testcontainers::{
        ContainerAsync, ImageExt,
        core::{WaitFor, logs::consumer::logging_consumer::LoggingConsumer},
        runners::AsyncRunner,
    };
    use testcontainers_modules::k3s::{K3s, KUBE_SECURE_PORT};

    async fn create_cluster_and_infra() -> (ContainerAsync<K3s>, KubernetesInfrastructure, TempDir)
    {
        let _ = env_logger::builder().is_test(true).try_init();

        let tempdir = tempfile::tempdir().unwrap();
        let config_mount = tempdir.path().to_path_buf();

        let k3s_instance = K3s::default()
                .with_conf_mount(&config_mount)
                .with_privileged(true)
                .with_ready_conditions(vec![WaitFor::message_on_stderr(
                        r#""QuotaMonitor created object count evaluator" resource="ingressroutetcps.traefik.io""#,
                )])
                .with_startup_timeout(std::time::Duration::from_mins(2))
                .with_log_consumer(
                    LoggingConsumer::new()
                    .with_stdout_level(log::Level::Trace)
                    .with_stderr_level(log::Level::Trace),
                )
                .start()
                .await
                .unwrap();

        let mapped_port = k3s_instance
            .get_host_port_ipv4(KUBE_SECURE_PORT.as_u16())
            .await
            .unwrap();

        let config = k3s_instance.image().read_kube_config().unwrap();

        let config_file = tempdir.path().join("k3s-mapped.yaml");
        let config = config.replace(
            "server: https://127.0.0.1:6443",
            &format!("server: https://127.0.0.1:{mapped_port}"),
        );
        std::fs::write(&config_file, config).unwrap();

        let infra = KubernetesInfrastructure::new(PREvantConfig {
            runtime: Runtime::Kubernetes(KubernetesRuntimeConfig {
                kube_config: Some(config_file),
                ..Default::default()
            }),
            ..Default::default()
        });

        (k3s_instance, infra, tempdir)
    }

    #[tokio::test]
    async fn fetch_backed_up_app() {
        let _ = env_logger::builder().is_test(true).try_init();

        let (_k3s, infra, _tempdir) = create_cluster_and_infra().await;

        let app_name = AppName::master();
        let unit = AppDeploymentBuilder::init(
            app_name.clone(),
            vec![blueprint_service!("http1", "nginx")],
            None,
        )
        .finish()
        .unwrap();

        let deploy_result = infra
            .deploy_services(&unit, &Default::default())
            .await
            .map_err(AppsError::from);
        assert_eq!(
            deploy_result.map(|app| app
                .services
                .into_iter()
                .map(|s| s.blueprint_config)
                .collect()),
            Ok(vec![blueprint_service!("http1", "nginx")])
        );

        let backup_payload = infra
            .fetch_app_as_backup_based_infrastructure_payload(&app_name)
            .await
            .unwrap()
            .unwrap();
        infra
            .delete_infrastructure_objects_partially(&app_name, &backup_payload)
            .await
            .unwrap();

        let fetch_result = infra.fetch_apps().await.map_err(AppsError::from);
        assert_eq!(
            fetch_result
                .as_ref()
                .map(|apps| { apps.values().filter_map(|app| app.created_at).count() }),
            Ok(1)
        );
        assert_eq!(
            fetch_result
                .and_then(move |mut apps| apps.remove(&app_name).ok_or_else(|| {
                    AppsError::AppNotFound {
                        app_name: app_name.clone(),
                    }
                }))
                .map(|app| app.services),
            Ok(vec![]),
        );
    }

    #[tokio::test]
    async fn fetch_regular_apps() {
        let _ = env_logger::builder().is_test(true).try_init();

        let (_k3s, infra, _tempdir) = create_cluster_and_infra().await;

        let app_name = AppName::master();
        let unit = AppDeploymentBuilder::init(
            app_name.clone(),
            vec![blueprint_service!("http1", "nginx")],
            None,
        )
        .finish()
        .unwrap();

        let deploy_result = infra
            .deploy_services(&unit, &Default::default())
            .await
            .map_err(AppsError::from);
        assert_eq!(
            deploy_result.map(|app| app
                .services
                .into_iter()
                .map(|s| s.blueprint_config)
                .collect()),
            Ok(vec![blueprint_service!("http1", "nginx")])
        );

        let fetch_result = infra.fetch_apps().await.map_err(AppsError::from);
        assert_eq!(
            fetch_result
                .as_ref()
                .map(|apps| { apps.values().filter_map(|app| app.created_at).count() }),
            Ok(1)
        );
        assert_eq!(
            fetch_result
                .and_then(move |mut apps| apps.remove(&app_name).ok_or_else(|| {
                    AppsError::AppNotFound {
                        app_name: app_name.clone(),
                    }
                }))
                .map(|app| app
                    .services
                    .into_iter()
                    .map(|s| s.blueprint_config)
                    .collect()),
            Ok(vec![blueprint_service!("http1", "nginx")])
        );
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case::only_bootstrapping(
        vec![],
        (ContainerType::ApplicationCompanion, Image::from_str("traefik/whoami").unwrap())
    )]
    #[case::merging_user_payload(
        vec![blueprint_service!("whoami", "traefik/whoami:v1.11.0")],
        (ContainerType::Instance, Image::from_str("traefik/whoami:v1.11.0").unwrap())
    )]
    async fn bootstrap_application(
        #[case] service_configs: Vec<domain::app_blueprints::ServiceConfig>,
        #[case] (expected_container_type, expected_image): (ContainerType, Image),
    ) -> Result<()> {
        let _ = env_logger::builder().is_test(true).try_init();

        let (_k3s, infra, _tempdir) = create_cluster_and_infra().await;

        let app_name = AppName::master();
        let (unit, _) = StaticBootstrapCompanion {
            app_name: app_name.clone(),
            service_configs,
            ..Default::default()
        }
        .bootstrap(&infra)
        .await?;

        let app = infra.fetch_app(&app_name).await?;

        assert_eq!(
            Some(vec![("whoami", &expected_container_type, &expected_image)]),
            app.as_ref().map(|app| {
                app.services
                    .iter()
                    .map(|service| {
                        (
                            service.blueprint_config.service_name.as_str(),
                            &service.service_type,
                            &service.blueprint_config.image,
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        );

        let payload = K8sDeploymentUnit::fetch(infra.client().await?, &app_name)
            .await?
            .without_managed_data()
            .without_date_annotations()
            .to_json_vec();

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!([
                {},
                {},
                {},
                {},
                {
                  "apiVersion": "traefik.containo.us/v1alpha1",
                  "kind": "IngressRoute",
                  "metadata": {
                    "annotations": {
                      "com.aixigo.preview.servant.app-name": "master",
                      "com.aixigo.preview.servant.container-type": expected_container_type,
                      "com.aixigo.preview.servant.service-name": "whoami",
                      "traefik.ingress.kubernetes.io/router.entrypoints": "web"
                    },
                    "name": "whoami",
                    "namespace": "master"
                  },
                  "spec": {
                    "routes": [
                      {
                        "kind": "Rule",
                        "match": "PathPrefix(`/master/my-route/`)",
                        "middlewares": [{
                            "name": "whoami-middleware",
                        }],
                        "services": [
                          {
                            "kind": "Service",
                            "name": "whoami",
                            "port": 2001
                          }
                        ]
                      }
                    ]
                  }
                },
                {
                  "apiVersion": "traefik.containo.us/v1alpha1",
                  "kind": "Middleware",
                  "metadata": {
                    "name": "whoami-middleware",
                    "namespace": "master"
                  },
                  "spec": {
                    "stripPrefix": {
                      "prefixes": [
                        "/master/my-route/"
                      ]
                    }
                  }
                }
            ])
        );

        // Redeploy again to check if the operation is idempotent
        infra.deploy_services(&unit, &Default::default()).await?;

        let payload_2 = K8sDeploymentUnit::fetch(infra.client().await?, &app_name)
            .await?
            .without_managed_data()
            .without_date_annotations()
            .to_json_vec();
        assert_json_diff::assert_json_eq!(payload, payload_2);

        Ok(())
    }

    #[tokio::test]
    async fn bootstrap_application_without_deployment() -> Result<()> {
        let _ = env_logger::builder().is_test(true).try_init();

        let (_k3s, infra, _tempdir) = create_cluster_and_infra().await;

        let app_name = AppName::master();
        let (unit, _) = StaticBootstrapCompanion {
            generate_whoami_deployment: false,
            app_name: app_name.clone(),
            ..Default::default()
        }
        .bootstrap(&infra)
        .await?;

        let payload = K8sDeploymentUnit::fetch(infra.client().await?, &app_name)
            .await?
            .without_managed_data()
            .to_json_vec();

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!([
                {},
                {},
                {
                  "apiVersion": "traefik.containo.us/v1alpha1",
                  "kind": "IngressRoute",
                  "metadata": {
                    "annotations": {
                      "com.aixigo.preview.servant.app-name": "master",
                      "traefik.ingress.kubernetes.io/router.entrypoints": "web"
                    },
                    "name": "whoami",
                    "namespace": "master"
                  },
                  "spec": {
                    "routes": [
                      {
                        "kind": "Rule",
                        "match": "PathPrefix(`/master/my-route/`)",
                        "middlewares": [{
                            "name": "whoami-middleware",
                        }],
                        "services": [
                          {
                            "kind": "Service",
                            "name": "whoami",
                            "port": 2001
                          }
                        ]
                      }
                    ]
                  }
                },
                {
                  "apiVersion": "traefik.containo.us/v1alpha1",
                  "kind": "Middleware",
                  "metadata": {
                    "name": "whoami-middleware",
                    "namespace": "master"
                  },
                  "spec": {
                    "stripPrefix": {
                      "prefixes": [
                        "/master/my-route/"
                      ]
                    }
                  }
                }
            ])
        );

        // Redeploy again to check if the operation is idempotent
        infra.deploy_services(&unit, &Default::default()).await?;

        let payload_2 = K8sDeploymentUnit::fetch(infra.client().await?, &app_name)
            .await?
            .without_managed_data()
            .to_json_vec();
        assert_json_diff::assert_json_eq!(payload, payload_2);

        Ok(())
    }

    #[tokio::test]
    async fn bootstrap_application_and_update() -> Result<()> {
        let _ = env_logger::builder().is_test(true).try_init();

        let (_k3s, infra, _tempdir) = create_cluster_and_infra().await;

        let app_name = AppName::master();
        StaticBootstrapCompanion {
            app_name: app_name.clone(),
            ..Default::default()
        }
        .bootstrap(&infra)
        .await?;

        let app = infra.fetch_app(&app_name).await?;

        assert_eq!(
            Some(vec![(
                "whoami",
                &ContainerType::ApplicationCompanion,
                &Image::from_str("traefik/whoami").unwrap()
            )]),
            app.as_ref().map(|app| {
                app.services
                    .iter()
                    .map(|service| {
                        (
                            service.blueprint_config.service_name.as_str(),
                            &service.service_type,
                            &service.blueprint_config.image,
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        );

        let (unit, _) = StaticBootstrapCompanion {
            app_name: app_name.clone(),
            service_configs: vec![blueprint_service!("whoami", "traefik/whoami:v1.11.0")],
            ..Default::default()
        }
        .bootstrap(&infra)
        .await?;

        let app = infra.fetch_app(&app_name).await?;

        assert_eq!(
            Some(vec![(
                "whoami",
                &ContainerType::Instance,
                &Image::from_str("traefik/whoami:v1.11.0").unwrap()
            )]),
            app.as_ref().map(|app| {
                app.services
                    .iter()
                    .map(|service| {
                        (
                            service.blueprint_config.service_name.as_str(),
                            &service.service_type,
                            &service.blueprint_config.image,
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        );

        let payload = K8sDeploymentUnit::fetch(infra.client().await?, &app_name)
            .await?
            .without_managed_data()
            .without_date_annotations()
            .to_json_vec();

        // Redeploy again to check if the operation is idempotent
        infra.deploy_services(&unit, &Default::default()).await?;

        let payload_2 = K8sDeploymentUnit::fetch(infra.client().await?, &app_name)
            .await?
            .without_managed_data()
            .without_date_annotations()
            .to_json_vec();
        assert_json_diff::assert_json_eq!(payload, payload_2);

        Ok(())
    }

    #[tokio::test]
    async fn deploy_application_twice() -> Result<()> {
        let _ = env_logger::builder().is_test(true).try_init();

        let (_k3s, infra, _tempdir) = create_cluster_and_infra().await;

        let app_name = AppName::master();
        let unit = AppDeploymentBuilder::init(
            app_name.clone(),
            vec![
                blueprint_service!(
                    "nextcloud",
                    "nextcloud",
                    env = (
                        "MYSQL_DATABASE" => "example",
                        "MYSQL_USER" => "example-user",
                        "MYSQL_PASSWORD" => "my_cool_secret",
                        "MYSQL_HOST" => "db"
                    )
                ),
                blueprint_service!(
                    "db",
                    "mariadb",
                    env = (
                        "MARIADB_ROOT_PASSWORD" => "example",
                        "MARIADB_USER" => "example-user",
                        "MARIADB_PASSWORD" => "my_cool_secret",
                        "MARIADB_DATABASE" => "example-database"
                    )
                ),
            ],
            None,
        )
        .finish()?;

        infra.deploy_services(&unit, &Default::default()).await?;

        let app = infra.fetch_app(&app_name).await?;

        assert_eq!(
            Some(vec![
                (
                    "db",
                    &ContainerType::Instance,
                    &Image::from_str("mariadb").unwrap()
                ),
                (
                    "nextcloud",
                    &ContainerType::Instance,
                    &Image::from_str("nextcloud").unwrap()
                ),
            ]),
            app.as_ref().map(|app| {
                app.services
                    .iter()
                    .map(|service| {
                        (
                            service.blueprint_config.service_name.as_str(),
                            &service.service_type,
                            &service.blueprint_config.image,
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        );

        let payload = K8sDeploymentUnit::fetch(infra.client().await?, &app_name)
            .await?
            .without_managed_data()
            .without_date_annotations()
            .to_json_vec();

        // Redeploy again to check if the operation is idempotent
        infra.deploy_services(&unit, &Default::default()).await?;

        let payload_2 = K8sDeploymentUnit::fetch(infra.client().await?, &app_name)
            .await?
            .without_managed_data()
            .without_date_annotations()
            .to_json_vec();
        assert_json_diff::assert_json_eq!(payload, payload_2);

        Ok(())
    }

    /// A fake bootstrapping implementation for testing purposes
    struct StaticBootstrapCompanion {
        generate_whoami_deployment: bool,
        app_name: AppName,
        service_configs: Vec<domain::app_blueprints::ServiceConfig>,
        app_to_replicate_from: Option<(AppName, App)>,
    }

    impl Default for StaticBootstrapCompanion {
        fn default() -> Self {
            Self {
                generate_whoami_deployment: true,
                app_name: AppName::master(),
                service_configs: Vec::new(),
                app_to_replicate_from: None,
            }
        }
    }

    impl StaticBootstrapCompanion {
        async fn bootstrap(
            self,
            infra: &KubernetesInfrastructure,
        ) -> Result<(DeploymentUnit, App)> {
            let unit = AppDeploymentBuilder::init(
                self.app_name.clone(),
                self.service_configs.clone(),
                None,
            )
            .with_app_to_replicate_from(
                self.app_to_replicate_from
                    .as_ref()
                    .map(|(app_name, _)| app_name.clone()),
            )
            .with_static_companions(std::iter::empty())
            .resolve_apps::<Infallible, _>(|app_name| {
                Ok(self
                    .app_to_replicate_from
                    .as_ref()
                    .filter(|(o, _)| app_name == *o)
                    .map(|(_, app)| app.clone()))
            })
            .await?
            .resolve_image_manifests::<Infallible, _>(async |_| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<Infallible, _>(async || Ok(None))
            .await?
            .resolve_infrastructure_template_data::<Infallible, _>(async |_app_name| Ok(None))
            .await?
            .bootstrap_companions::<anyhow::Error, _>(self)
            .await?
            .finish()?;

            let app = infra.deploy_services(&unit, &Default::default()).await?;

            Ok((unit, app))
        }
    }

    #[async_trait::async_trait]
    impl domain::app_deployment::BootstrapCompanions for StaticBootstrapCompanion {
        type Error = anyhow::Error;

        async fn bootstrap_companions_with_raw_elements(
            &self,
            context: BootstrapCompanionsWithRawElementsContext<'_>,
            _template_data: &TemplateData,
        ) -> Result<BootstrappedCompanions, Self::Error> {
            let output = [
                if self.generate_whoami_deployment {
                    r#"
                        apiVersion: apps/v1
                        kind: Deployment
                        metadata:
                          name: whoami
                        spec:
                          selector:
                            matchLabels:
                              app: whoami
                          template:
                            metadata:
                              labels:
                                app: whoami
                            spec:
                              containers:
                              - name: whoami
                                image: traefik/whoami
                                args:
                                - --port=2001
                                - --name=iamfoo
                                ports:
                                - containerPort: 2001
                    "#
                } else {
                    ""
                }
                .as_bytes(),
                if self.generate_whoami_deployment {
                    r#"
                        apiVersion: v1
                        kind: Service
                        metadata:
                          name: whoami
                        spec:
                          selector:
                            app: whoami
                          ports:
                          - port: 2001
                            targetPort: 2001
                    "#
                } else {
                    ""
                }
                .as_bytes(),
                r#"
                    apiVersion: networking.k8s.io/v1
                    kind: Ingress
                    metadata:
                      name: whoami
                      annotations:
                        nginx.ingress.kubernetes.io/use-regex: true
                        nginx.ingress.kubernetes.io/rewrite-target: /$2
                    spec:
                      ingressClassName: nginx
                      rules:
                      - http:
                          paths:
                          - path: /my-route
                            pathType: Prefix
                            backend:
                              service:
                                name: whoami
                                port:
                                  number: 2001
                "#
                .as_bytes(),
            ];

            let k8s_deployment_unit =
                K8sDeploymentUnit::parse_from_log_streams(context.app_name, output).await?;

            Ok(BootstrappedCompanions::try_from(k8s_deployment_unit)?)
        }

        fn update_raw_elements(
            &self,
            context: MergeRawElementsContext<'_>,
            raw_elements: Vec<RawInfrastructureElement>,
        ) -> Result<Vec<RawInfrastructureElement>> {
            Ok(K8sDeploymentUnit::parse_from_json(
                &self.app_name,
                raw_elements.into_iter().map(serde_json::Value::from),
            )?
            .update_with_merge_context(context)?
            .to_json_vec()
            .into_iter()
            .map(RawInfrastructureElement::from)
            .collect::<Vec<_>>())
        }
    }
}
