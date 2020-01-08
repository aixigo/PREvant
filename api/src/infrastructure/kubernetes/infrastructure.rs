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
use super::super::{APP_NAME_LABEL, CONTAINER_TYPE_LABEL, IMAGE_LABEL, SERVICE_NAME_LABEL};
use super::payloads::{
    deployment_payload, deployment_replicas_payload, ingress_route_payload, middleware_payload,
    namespace_payload, secrets_payload, service_payload,
};
use crate::config::ContainerConfig;
use crate::infrastructure::Infrastructure;
use crate::models::service::{ContainerType, Service, ServiceError, ServiceStatus};
use crate::models::{Image, ServiceBuilder, ServiceBuilderError, ServiceConfig};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use failure::Error;
use futures::future::join_all;
use k8s_openapi::api::apps::v1::{DeploymentSpec, DeploymentStatus};
use kube::{
    api::{Api, DeleteParams, ListParams, LogParams, Object, PatchParams, PostParams, RawApi},
    client::APIClient,
    config::Configuration,
    Error as KubeError, ErrorResponse,
};
use multimap::MultiMap;
use reqwest::{Certificate, Client};
use secstr::SecUtf8;
use std::collections::BTreeMap;
use std::convert::{From, TryFrom};
use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;

type Deployment = Object<DeploymentSpec, DeploymentStatus>;

pub struct KubernetesInfrastructure {
    cluster_endpoint: Url,
    cluster_ca: Option<Certificate>,
    cluster_token: Option<SecUtf8>,
}

#[derive(Debug, Fail)]
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
}

impl KubernetesInfrastructure {
    pub fn new(
        cluster_endpoint: Url,
        cluster_ca: Option<Certificate>,
        cluster_token: Option<SecUtf8>,
    ) -> Self {
        KubernetesInfrastructure {
            cluster_endpoint,
            cluster_ca,
            cluster_token,
        }
    }

    fn client(&self) -> APIClient {
        use reqwest::header::{self, HeaderValue};
        let mut headers = header::HeaderMap::new();

        if let Some(token) = &self.cluster_token {
            let token_header_value =
                HeaderValue::from_str(&format!("Bearer {}", token.unsecure())).unwrap();
            headers.insert(header::AUTHORIZATION, token_header_value);
        }

        let mut client_builder = Client::builder().default_headers(headers);

        if let Some(ca) = &self.cluster_ca {
            client_builder = client_builder.add_root_certificate(ca.clone());
        }

        let client = client_builder
            .build()
            .expect("Should be able to create client");

        let mut endpoint = self.cluster_endpoint.to_string();
        if endpoint.ends_with('/') {
            endpoint.pop();
        }

        let configuration =
            Configuration::with_default_ns(endpoint, client, String::from("default"));

        APIClient::new(configuration)
    }

    async fn create_service_from(
        &self,
        deployment: Deployment,
    ) -> Result<Service, KubernetesInfrastructureError> {
        let namespace = deployment
            .metadata
            .namespace
            .clone()
            .unwrap_or("".to_string());
        let mut builder = ServiceBuilder::try_from(deployment)?;

        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}={}",
            APP_NAME_LABEL,
            builder.current_app_name().unwrap_or("".to_string()),
            SERVICE_NAME_LABEL,
            builder.current_service_name().unwrap_or("".to_string()),
        ));
        if let Some(pod) = Api::v1Pod(self.client())
            .within(&namespace)
            .list(&p)
            .await?
            .items
            .into_iter()
            .next()
        {
            if let Some(container) = pod.spec.containers.first() {
                builder = builder.started_at(
                    pod.status
                        .as_ref()
                        .map(|s| s.start_time.as_ref())
                        .flatten()
                        .map(|t| t.0)
                        .unwrap_or(Utc::now()),
                );

                if let Some(image) = container.image.as_ref().map(|image| {
                    Image::from_str(image)
                        .expect("Kubernetes API should provide valid image string")
                }) {
                    builder = builder.image(image);
                }

                if let Some(ip) = pod.status.as_ref().map(|pod| pod.pod_ip.as_ref()).flatten() {
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
        let futures = Api::v1Deployment(self.client())
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

        match Api::v1Deployment(self.client())
            .list(&p)
            .await?
            .items
            .into_iter()
            .next()
            .map(|deployment| self.create_service_from(deployment))
        {
            None => return Ok(None),
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
        Api::v1Service(self.client())
            .within(&app_name)
            .create(
                &PostParams::default(),
                service_payload(app_name, service_config).into_bytes(),
            )
            .await?;

        let request = RawApi::customResource("ingressroutes")
            .group("traefik.containo.us")
            .version("v1alpha1")
            .within(&app_name)
            .create(
                &PostParams::default(),
                ingress_route_payload(app_name, service_config).into_bytes(),
            )?;

        self.client().request_text(request).await?;

        let request = RawApi::customResource("middlewares")
            .group("traefik.containo.us")
            .version("v1alpha1")
            .within(&app_name)
            .create(
                &PostParams::default(),
                middleware_payload(app_name, service_config).into_bytes(),
            )?;

        self.client().request_text(request).await?;

        Ok(())
    }

    async fn create_namespace_if_necessary(
        &self,
        app_name: &String,
    ) -> Result<(), KubernetesInfrastructureError> {
        match Api::v1Namespace(self.client())
            .create(
                &PostParams::default(),
                namespace_payload(app_name).into_bytes(),
            )
            .await
        {
            Ok(result) => {
                debug!("Successfully created namespace {}", result.metadata.name);
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

        match Api::v1Deployment(self.client())
            .within(&app_name)
            .create(
                &PostParams::default(),
                deployment_payload(app_name, service_config, container_config).into_bytes(),
            )
            .await
        {
            Ok(result) => {
                debug!("Successfully deployed {}", result.metadata.name);
                self.post_service_and_custom_resource_definitions(app_name, service_config)
                    .await?;
                Ok(service_config)
            }

            Err(KubeError::Api(ErrorResponse { code, .. })) if code == 409 => {
                Api::v1Deployment(self.client())
                    .within(&app_name)
                    .patch(
                        &format!("{}-{}-deployment", app_name, service_config.service_name()),
                        &PatchParams::default(),
                        deployment_payload(app_name, service_config, container_config).into_bytes(),
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

        match Api::v1Secret(self.client())
            .within(&app_name)
            .create(
                &PostParams::default(),
                secrets_payload(app_name, service_config, volumes).into_bytes(),
            )
            .await
        {
            Ok(result) => {
                debug!("Successfully deployed {}", result.metadata.name);
                Ok(())
            }
            Err(KubeError::Api(ErrorResponse { code, .. })) if code == 409 => {
                Api::v1Secret(self.client())
                    .within(&app_name)
                    .patch(
                        &format!("{}-{}-secret", app_name, service_config.service_name()),
                        &PatchParams::default(),
                        secrets_payload(app_name, service_config, volumes).into_bytes(),
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

    async fn stop_service<'a>(
        &self,
        app_name: &String,
        service: &'a Service,
    ) -> Result<&'a Service, KubernetesInfrastructureError> {
        Api::v1Deployment(self.client())
            .within(&service.app_name())
            .delete(
                &format!("{}-{}-deployment", app_name, service.service_name()),
                &DeleteParams::default(),
            )
            .await?;
        Api::v1Service(self.client())
            .within(&service.app_name())
            .delete(service.service_name(), &DeleteParams::default())
            .await?;
        let request = RawApi::customResource("ingressroutes")
            .group("traefik.containo.us")
            .version("v1alpha1")
            .within(&service.app_name())
            .delete(
                &format!("{}-{}-ingress-route", app_name, service.service_name()),
                &DeleteParams::default(),
            )?;
        self.client().request_text(request).await?;
        let request = RawApi::customResource("middlewares")
            .group("traefik.containo.us")
            .version("v1alpha1")
            .within(&service.app_name())
            .delete(
                &format!("{}-{}-middleware", app_name, service.service_name()),
                &DeleteParams::default(),
            )?;
        self.client().request_text(request).await?;

        Ok(service)
    }
}

#[async_trait]
impl Infrastructure for KubernetesInfrastructure {
    async fn get_services(&self) -> Result<MultiMap<String, Service>, Error> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!("{},{}", APP_NAME_LABEL, SERVICE_NAME_LABEL));

        let mut apps = MultiMap::new();
        for deployment in Api::v1Deployment(self.client())
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
        app_name: &String,
        configs: &Vec<ServiceConfig>,
        container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
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

    async fn stop_services(&self, app_name: &String) -> Result<Vec<Service>, Error> {
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

        Api::v1Namespace(self.client())
            .delete(app_name, &DeleteParams::default())
            .await?;

        Ok(services)
    }

    async fn get_configs_of_app(&self, _app_name: &String) -> Result<Vec<ServiceConfig>, Error> {
        let mut configs = Vec::new();
        for service in self.get_services_of_app(_app_name).await? {
            match service.container_type() {
                ContainerType::ApplicationCompanion | ContainerType::ServiceCompanion => continue,
                _ => {}
            };

            let image = service
                .image()
                .cloned()
                .expect("Service should contain image");
            let mut service_config = ServiceConfig::new(service.service_name().clone(), image);

            if let Some(port) = service.port() {
                service_config.set_port(port);
            }

            configs.push(service_config);
        }

        Ok(configs)
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
        let pod = match Api::v1Pod(self.client())
            .within(app_name)
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

        let logs = Api::v1Pod(self.client())
            .within(&app_name)
            .log(&pod.metadata.name, &p)
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
                log_line += "\n";
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

        Api::v1Deployment(self.client())
            .within(&app_name)
            .patch(
                &format!("{}-{}-deployment", app_name, service_name),
                &PatchParams::default(),
                deployment_replicas_payload(app_name, &service, replicas).into_bytes(),
            )
            .await?;

        Ok(Some(service))
    }
}

impl TryFrom<Deployment> for ServiceBuilder {
    type Error = KubernetesInfrastructureError;

    fn try_from(deployment: Deployment) -> Result<Self, Self::Error> {
        let mut builder = ServiceBuilder::new().id(deployment.metadata.name.clone());

        let labels = deployment.metadata.labels;
        builder = match labels.get(APP_NAME_LABEL) {
            Some(app_name) => builder.app_name(app_name.clone()),
            None => {
                return Err(KubernetesInfrastructureError::MissingAppNameLabel {
                    deployment_name: deployment.metadata.name,
                });
            }
        };

        builder = match labels.get(SERVICE_NAME_LABEL) {
            Some(service_name) => builder.service_name(service_name.clone()),
            None => {
                return Err(KubernetesInfrastructureError::MissingServiceNameLabel {
                    deployment_name: deployment.metadata.name,
                });
            }
        };

        if let Some(lb) = labels.get(CONTAINER_TYPE_LABEL) {
            builder = builder.container_type(lb.parse::<ContainerType>()?);
        };

        builder = builder.service_status(
            deployment
                .spec
                .replicas
                .map(|replicas| {
                    if replicas <= 0 {
                        ServiceStatus::Paused
                    } else {
                        ServiceStatus::Running
                    }
                })
                .unwrap_or(ServiceStatus::Paused),
        );

        if let Some(image) = deployment
            .metadata
            .annotations
            .get(IMAGE_LABEL)
            .map(|image| {
                Image::from_str(image).expect("Kubernetes API should provide valid image string")
            })
        {
            builder = builder.image(image);
        }

        Ok(builder)
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
    use kube::api::{ObjectMeta, TypeMeta};
    use std::collections::BTreeMap;

    macro_rules! deployment_object {
        ($deployment_name:expr, $app_name:expr, $service_name:expr, $container_type:expr) => {{
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

            Object {
                types: TypeMeta::default(),
                metadata: ObjectMeta {
                    name: String::from($deployment_name),
                    namespace: None,
                    labels,
                    annotations: BTreeMap::new(),
                    resourceVersion: None,
                    ownerReferences: vec![],
                    uid: None,
                    generation: None,
                    generateName: None,
                    initializers: None,
                    finalizers: vec![],
                    creation_timestamp: None,
                    deletion_timestamp: None,
                },
                spec: DeploymentSpec::default(),
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
            None
        );

        let service = ServiceBuilder::try_from(deployment)
            .unwrap()
            .id("same-random-id".to_string())
            .started_at(Utc::now())
            .build()
            .unwrap();

        // TODO: deployment name generation
        assert_eq!(service.app_name(), &String::from("master"));
        assert_eq!(service.service_name(), &String::from("nginx"));
    }

    #[test]
    fn should_parse_service_from_deployment_spec_without_container_type() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            None
        );

        let service = ServiceBuilder::try_from(deployment)
            .unwrap()
            .id("same-random-id".to_string())
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
            Some(String::from("replica"))
        );

        let service = ServiceBuilder::try_from(deployment)
            .unwrap()
            .id("same-random-id".to_string())
            .started_at(Utc::now())
            .build()
            .unwrap();

        assert_eq!(service.container_type(), &ContainerType::Replica);
    }

    #[test]
    fn should_not_parse_service_from_deployment_spec_missing_app_name_label() {
        let deployment =
            deployment_object!("master-nginx", None, Some(String::from("nginx")), None);

        let err = match ServiceBuilder::try_from(deployment) {
            Ok(_) => panic!("Should not be parseble"),
            Err(err) => err,
        };

        match err {
            // TODO: deployment name generation
            KubernetesInfrastructureError::MissingAppNameLabel { deployment_name } => {
                assert_eq!(deployment_name, "master-nginx");
            }
            _ => panic!("unexpected error"),
        };
    }

    #[test]
    fn should_not_parse_service_from_deployment_spec_missing_service_name_label() {
        let deployment =
            deployment_object!("master-nginx", Some(String::from("master")), None, None);

        let err = match ServiceBuilder::try_from(deployment) {
            Ok(_) => panic!("Should not be parseble"),
            Err(err) => err,
        };

        match err {
            // TODO: deployment name generation
            KubernetesInfrastructureError::MissingServiceNameLabel { deployment_name } => {
                assert_eq!(deployment_name, "master-nginx");
            }
            _ => panic!("unexpected error"),
        };
    }

    #[test]
    fn should_not_parse_service_from_deployment_spec_invalid_container_type() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("abc"))
        );

        let err = match ServiceBuilder::try_from(deployment) {
            Ok(_) => panic!("Should not be parseble"),
            Err(err) => err,
        };

        match err {
            KubernetesInfrastructureError::UnknownServiceType { unknown_label } => {
                assert_eq!(unknown_label, "abc");
            }
            _ => panic!("unexpected error"),
        };
    }
}
