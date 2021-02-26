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
};
use super::payloads::{
    deployment_payload, deployment_replicas_payload, ingress_route_payload, middleware_payload,
    namespace_payload, secrets_payload, service_payload, IngressRoute, Middleware,
};
use crate::config::ContainerConfig;
use crate::infrastructure::Infrastructure;
use crate::models::service::{ContainerType, Service, ServiceError, ServiceStatus};
use crate::models::{Environment, Image, ServiceBuilder, ServiceBuilderError, ServiceConfig};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use failure::Error;
use futures::future::join_all;
use k8s_openapi::api::{
    apps::v1::Deployment as V1Deployment, core::v1::Namespace as V1Namespace,
    core::v1::Pod as V1Pod, core::v1::Secret as V1Secret, core::v1::Service as V1Service,
};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1beta1::CustomResourceDefinition;
use kube::{
    api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams},
    client::Client,
    config::{Config, Der},
    error::{Error as KubeError, ErrorResponse},
};
use multimap::MultiMap;
use openssl::error::ErrorStack;
use openssl::x509::X509;
use secstr::SecUtf8;
use std::collections::BTreeMap;
use std::convert::{From, TryFrom};
use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;

pub struct KubernetesInfrastructure {
    cluster_endpoint: Url,
    cluster_ca: Option<Vec<X509>>,
    cluster_token: Option<SecUtf8>,
}

#[derive(Debug, Fail, PartialEq)]
pub enum KubernetesInfrastructureError {
    #[fail(
        display = "Unexpected Kubernetes interaction error: {}",
        internal_message
    )]
    UnexpectedError { internal_message: String },
    #[fail(
        display = "The deployment {} does not provide a label for service name.",
        deployment_name
    )]
    MissingServiceNameLabel { deployment_name: String },
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
    #[fail(display = "Could not convert certificate: {}", internal_message)]
    CertificateError { internal_message: String },
}

impl KubernetesInfrastructure {
    pub fn new(
        cluster_endpoint: Url,
        cluster_ca: Option<Vec<X509>>,
        cluster_token: Option<SecUtf8>,
    ) -> Self {
        KubernetesInfrastructure {
            cluster_endpoint,
            cluster_ca,
            cluster_token,
        }
    }

    fn client(&self) -> Result<Client, KubernetesInfrastructureError> {
        use reqwest::header::{self, HeaderValue};
        let mut headers = header::HeaderMap::new();
        let mut root_cert: Option<Vec<Der>> = None;

        if let Some(token) = &self.cluster_token {
            let token_header_value =
                HeaderValue::from_str(&format!("Bearer {}", token.unsecure())).unwrap();
            headers.insert(header::AUTHORIZATION, token_header_value);
        }

        if let Some(certs) = &self.cluster_ca {
            let result: Result<Vec<Der>, ErrorStack> = certs
                .into_iter()
                .map(|cert| Ok(Der(cert.to_der()?)))
                .collect();
            let ders = result.map_err(|err| KubernetesInfrastructureError::CertificateError {
                internal_message: format!("{}", err),
            })?;

            root_cert = Some(ders);
        }

        let mut configuration = Config::new(self.cluster_endpoint.clone());
        configuration.default_ns = String::from("default");
        configuration.root_cert = root_cert;
        configuration.headers = headers;

        Client::try_from(configuration).map_err(|err| {
            KubernetesInfrastructureError::UnexpectedError {
                internal_message: format!("Failed to create client: {}", err),
            }
        })
    }

    async fn create_service_from(
        &self,
        deployment: V1Deployment,
    ) -> Result<Service, KubernetesInfrastructureError> {
        let namespace = deployment
            .metadata
            .namespace
            .clone()
            .unwrap_or_else(|| "".to_string());
        let mut builder = ServiceBuilder::try_from(deployment)?;

        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}={}",
            APP_NAME_LABEL,
            builder
                .current_app_name()
                .map_or_else(|| "", |name| name.as_str()),
            SERVICE_NAME_LABEL,
            builder
                .current_config()
                .map_or_else(|| "", |config| config.service_name()),
        ));
        if let Some(pod) = Api::<V1Pod>::namespaced(self.client()?, &namespace)
            .list(&p)
            .await?
            .items
            .into_iter()
            .next()
        {
            if let Some(container) = pod.spec.as_ref().and_then(|spec| spec.containers.first()) {
                builder = builder.started_at(
                    pod.status
                        .as_ref()
                        .map(|s| s.start_time.as_ref())
                        .flatten()
                        .map(|t| t.0)
                        .unwrap_or_else(Utc::now),
                );

                if let Some(ip) = pod.status.as_ref().and_then(|pod| pod.pod_ip.as_ref()) {
                    let port = container
                        .ports
                        .as_ref()
                        .map(|ports| ports.first())
                        .flatten()
                        .map(|port| port.container_port as u16)
                        .unwrap_or(80u16);

                    builder = builder.endpoint(
                        IpAddr::from_str(&ip)
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
        app_name: &String,
    ) -> Result<Vec<Service>, KubernetesInfrastructureError> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}",
            APP_NAME_LABEL, app_name, SERVICE_NAME_LABEL
        ));

        let mut services = Vec::new();
        let futures = Api::<V1Deployment>::all(self.client()?)
            .list(&p)
            .await?
            .items
            .into_iter()
            .map(|deployment| self.create_service_from(deployment))
            .collect::<Vec<_>>();

        for create_service_result in join_all(futures).await {
            let service = match create_service_result {
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

    async fn get_service_of_app(
        &self,
        app_name: &String,
        service_name: &String,
    ) -> Result<Option<Service>, KubernetesInfrastructureError> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}={}",
            APP_NAME_LABEL, app_name, SERVICE_NAME_LABEL, service_name
        ));

        match Api::<V1Deployment>::all(self.client()?)
            .list(&p)
            .await?
            .items
            .into_iter()
            .next()
            .map(|deployment| self.create_service_from(deployment))
        {
            None => Ok(None),
            Some(service) => Ok(Some(service.await?)),
        }
    }

    async fn post_service_and_custom_resource_definitions(
        &self,
        app_name: &String,
        service_config: &ServiceConfig,
    ) -> Result<(), KubernetesInfrastructureError> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}={}",
            APP_NAME_LABEL,
            app_name,
            SERVICE_NAME_LABEL,
            service_config.service_name(),
        ));
        Api::namespaced(self.client()?, &app_name)
            .create(
                &PostParams::default(),
                &service_payload(app_name, service_config),
            )
            .await?;

        Api::namespaced(self.client()?, &app_name)
            .create(
                &PostParams::default(),
                &ingress_route_payload(app_name, service_config),
            )
            .await?;

        Api::namespaced(self.client()?, &app_name)
            .create(
                &PostParams::default(),
                &middleware_payload(app_name, service_config),
            )
            .await?;

        Ok(())
    }

    async fn create_crds_if_necessary(
        &self,
        _app_name: &String,
    ) -> Result<(), KubernetesInfrastructureError> {
        let pp = PatchParams::default();
        let api: Api<CustomResourceDefinition> = Api::all(self.client()?);
        api.patch(
            "ingressroutes.traefik.containo.us",
            &pp,
            &Patch::Merge(IngressRoute::crd()),
        )
        .await?;
        api.patch(
            "middlewares.traefik.containo.us",
            &pp,
            &Patch::Merge(Middleware::crd()),
        )
        .await?;
        Ok(())
    }

    async fn create_namespace_if_necessary(
        &self,
        app_name: &String,
    ) -> Result<(), KubernetesInfrastructureError> {
        match Api::all(self.client()?)
            .create(&PostParams::default(), &namespace_payload(app_name))
            .await
        {
            Ok(result) => {
                debug!(
                    "Successfully created namespace {}",
                    result.metadata.name.unwrap_or(String::from("<unknown>"))
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

    async fn deploy_service<'a>(
        &self,
        app_name: &String,
        service_config: &'a ServiceConfig,
        container_config: &ContainerConfig,
    ) -> Result<&'a ServiceConfig, KubernetesInfrastructureError> {
        if let Some(volumes) = service_config.volumes() {
            self.deploy_secret(app_name, service_config, volumes)
                .await?;
        }

        match Api::namespaced(self.client()?, &app_name)
            .create(
                &PostParams::default(),
                &deployment_payload(app_name, service_config, container_config),
            )
            .await
        {
            Ok(result) => {
                debug!(
                    "Successfully deployed {}",
                    result.metadata.name.unwrap_or(String::from("<unknown>"))
                );
                self.post_service_and_custom_resource_definitions(app_name, service_config)
                    .await?;
                Ok(service_config)
            }

            Err(KubeError::Api(ErrorResponse { code, .. })) if code == 409 => {
                Api::<V1Deployment>::namespaced(self.client()?, &app_name)
                    .patch(
                        &format!("{}-{}-deployment", app_name, service_config.service_name()),
                        &PatchParams::default(),
                        &Patch::Merge(deployment_payload(
                            app_name,
                            service_config,
                            container_config,
                        )),
                    )
                    .await?;
                Ok(service_config)
            }
            Err(e) => {
                error!("Cannot deploy service: {}", e);
                Err(e.into())
            }
        }
    }

    async fn deploy_secret(
        &self,
        app_name: &String,
        service_config: &ServiceConfig,
        volumes: &BTreeMap<PathBuf, String>,
    ) -> Result<(), KubernetesInfrastructureError> {
        debug!(
            "Deploying volumes as secrets for {} in app {}",
            service_config.service_name(),
            app_name
        );

        match Api::namespaced(self.client()?, &app_name)
            .create(
                &PostParams::default(),
                &secrets_payload(app_name, service_config, volumes),
            )
            .await
        {
            Ok(result) => {
                debug!(
                    "Successfully deployed {}",
                    result.metadata.name.unwrap_or(String::from("<unknown>"))
                );
                Ok(())
            }
            Err(KubeError::Api(ErrorResponse { code, .. })) if code == 409 => {
                Api::<V1Secret>::namespaced(self.client()?, &app_name)
                    .patch(
                        &format!("{}-{}-secret", app_name, service_config.service_name()),
                        &PatchParams::default(),
                        &Patch::Merge(secrets_payload(app_name, service_config, volumes)),
                    )
                    .await?;
                Ok(())
            }
            Err(e) => {
                error!("Cannot deploy secret: {}", e);
                Err(e.into())
            }
        }
    }

    async fn stop_service<'a, 'b: 'a>(
        &'b self,
        app_name: &String,
        service: &'a Service,
    ) -> Result<&'a Service, KubernetesInfrastructureError> {
        Api::<V1Deployment>::namespaced(self.client()?, &service.app_name())
            .delete(
                &format!("{}-{}-deployment", app_name, service.service_name()),
                &DeleteParams::default(),
            )
            .await?;
        Api::<V1Service>::namespaced(self.client()?, &service.app_name())
            .delete(service.service_name(), &DeleteParams::default())
            .await?;
        Api::<IngressRoute>::namespaced(self.client()?, &service.app_name())
            .delete(
                &format!("{}-{}-ingress-route", app_name, service.service_name()),
                &DeleteParams::default(),
            )
            .await?;
        Api::<Middleware>::namespaced(self.client()?, &service.app_name())
            .delete(
                &format!("{}-{}-middleware", app_name, service.service_name()),
                &DeleteParams::default(),
            )
            .await?;

        Ok(service)
    }
}

#[async_trait]
impl Infrastructure for KubernetesInfrastructure {
    async fn get_services(&self) -> Result<MultiMap<String, Service>, Error> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!("{},{}", APP_NAME_LABEL, SERVICE_NAME_LABEL));

        let mut apps = MultiMap::new();
        for deployment in Api::<V1Deployment>::all(self.client()?)
            .list(&p)
            .await?
            .items
            .into_iter()
        {
            let service = match self.create_service_from(deployment).await {
                Ok(service) => service,
                Err(e) => {
                    debug!("Deployment does not provide required data: {:?}", e);
                    continue;
                }
            };

            apps.insert(service.app_name().clone(), service);
        }

        Ok(apps)
    }

    async fn deploy_services(
        &self,
        _status_id: &String,
        app_name: &String,
        configs: &Vec<ServiceConfig>,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
        self.create_crds_if_necessary(app_name).await?;
        self.create_namespace_if_necessary(app_name).await?;

        let futures = configs
            .iter()
            .map(|config| self.deploy_service(app_name, config, container_config))
            .collect::<Vec<_>>();

        for deploy_result in join_all(futures).await {
            trace!("deployed {:?}", deploy_result);
            deploy_result?;
        }

        Ok(self.get_services_of_app(app_name).await?)
    }

    async fn stop_services(
        &self,
        _status_id: &String,
        app_name: &String,
    ) -> Result<Vec<Service>, Error> {
        let services = self.get_services_of_app(app_name).await?;
        if services.is_empty() {
            return Ok(services);
        }

        let futures = services
            .iter()
            .map(|service| self.stop_service(&app_name, &service))
            .collect::<Vec<_>>();

        for stop_service_result in join_all(futures).await {
            trace!("stopped: {:?}", stop_service_result);
            stop_service_result?;
        }

        Api::<V1Namespace>::all(self.client()?)
            .delete(app_name, &DeleteParams::default())
            .await?;

        Ok(services)
    }

    async fn get_logs(
        &self,
        app_name: &String,
        service_name: &String,
        from: &Option<DateTime<FixedOffset>>,
        limit: usize,
    ) -> Result<Option<Vec<(DateTime<FixedOffset>, String)>>, Error> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}={}",
            APP_NAME_LABEL, app_name, SERVICE_NAME_LABEL, service_name,
        ));
        let pod = match Api::<V1Pod>::namespaced(self.client()?, &app_name)
            .list(&p)
            .await?
            .into_iter()
            .next()
        {
            Some(pod) => pod,
            None => {
                return Ok(None);
            }
        };

        let mut p = LogParams::default();
        p.timestamps = true;
        p.since_seconds = from
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
            .filter(|since_seconds| since_seconds > &0);

        let logs = Api::<V1Pod>::namespaced(self.client()?, &app_name)
            .logs(&pod.metadata.name.unwrap(), &p)
            .await?;

        let logs = logs
            .split("\n")
            .enumerate()
            // Unfortunately,  API does not support head (also like docker, cf. https://github.com/moby/moby/issues/13096)
            // Until then we have to skip these log messages which is super slow…
            .filter(move |(index, _)| index < &limit)
            .filter(|(_, line)| !line.is_empty())
            .map(|(_, line)| {
                let mut iter = line.splitn(2, ' ').into_iter();
                let timestamp = iter.next().expect(
                    "This should never happen: kubernetes should return timestamps, separated by space",
                );

                let datetime =
                    DateTime::parse_from_rfc3339(&timestamp).expect("Expecting a valid timestamp");

                let mut log_line: String = iter.collect::<Vec<&str>>().join(" ");
                log_line.push('\n');
                (datetime, log_line)
            })
            .collect();

        Ok(Some(logs))
    }

    async fn change_status(
        &self,
        app_name: &String,
        service_name: &String,
        status: ServiceStatus,
    ) -> Result<Option<Service>, Error> {
        let (service, replicas) = match self.get_service_of_app(app_name, service_name).await? {
            Some(service) if service.status() == &status => return Ok(None),
            Some(service) => match status {
                ServiceStatus::Running => (service, 1),
                ServiceStatus::Paused => (service, 0),
            },
            None => return Ok(None),
        };

        Api::<V1Deployment>::namespaced(self.client()?, &app_name)
            .patch(
                &format!("{}-{}-deployment", app_name, service_name),
                &PatchParams::default(),
                &Patch::Merge(deployment_replicas_payload(app_name, &service, replicas)),
            )
            .await?;

        Ok(Some(service))
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
                .map(|spec| match (spec.paused, spec.replicas) {
                    (Some(true), _) => ServiceStatus::Paused,
                    (Some(false), Some(replicas)) if replicas <= 0 => ServiceStatus::Paused,
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
            let service_name = match labels.get(SERVICE_NAME_LABEL) {
                Some(service_name) => service_name,
                None => {
                    return Err(KubernetesInfrastructureError::MissingServiceNameLabel {
                        deployment_name: deployment_name.clone(),
                    });
                }
            };

            let image = annotations
                .get(IMAGE_LABEL)
                .map(|image| {
                    Image::from_str(image)
                        .expect("Kubernetes API should provide valid image string")
                })
                .ok_or_else(|| KubernetesInfrastructureError::MissingImageLabel {
                    deployment_name: deployment_name.clone(),
                })?;

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
    use std::collections::BTreeMap;

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
                    namespace: None,
                    labels: Some(labels),
                    annotations: Some(annotations),
                    resource_version: None,
                    owner_references: Some(vec![]),
                    uid: None,
                    generation: None,
                    generate_name: None,
                    initializers: None,
                    finalizers: Some(vec![]),
                    creation_timestamp: None,
                    deletion_timestamp: None,
                    cluster_name: None,
                    deletion_grace_period_seconds: None,
                    managed_fields: None,
                    self_link: None,
                },
                spec: Some(DeploymentSpec::default()),
                status: None,
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
    fn should_not_parse_service_from_deployment_spec_missing_service_name_label() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            None,
            Some(String::from("nginx")),
            None,
        );

        let err = ServiceBuilder::try_from(deployment).unwrap_err();
        assert_eq!(
            err,
            KubernetesInfrastructureError::MissingServiceNameLabel {
                deployment_name: "master-nginx".to_string()
            }
        );
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
