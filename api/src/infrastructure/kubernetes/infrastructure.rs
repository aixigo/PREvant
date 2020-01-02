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
use super::super::{APP_NAME_LABEL, CONTAINER_TYPE_LABEL, SERVICE_NAME_LABEL};
use super::payloads::{
    deployment_payload, ingress_route_payload, middleware_payload, namespace_payload,
    secrets_payload, service_payload,
};
use crate::config::ContainerConfig;
use crate::infrastructure::Infrastructure;
use crate::models::service::{ContainerType, Service, ServiceError, ServiceStatus};
use crate::models::{ServiceBuilder, ServiceBuilderError, ServiceConfig};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use failure::Error;
use k8s_openapi::api::apps::v1::{DeploymentSpec, DeploymentStatus};
use kube::{
    api::{Api, DeleteParams, ListParams, Log, LogParams, Object, PatchParams, PostParams, RawApi},
    client::APIClient,
    config::Configuration,
};
use multimap::MultiMap;
use reqwest::{Certificate, Client};
use secstr::SecUtf8;
use std::convert::{From, TryFrom};
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

    fn create_service_from(
        &self,
        deployment: Deployment,
    ) -> Result<Service, KubernetesInfrastructureError> {
        let namespace = deployment
            .metadata
            .namespace
            .clone()
            .unwrap_or("".to_string());
        let builder = ServiceBuilder::try_from(deployment)?;

        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}={}",
            APP_NAME_LABEL,
            builder.current_app_name().unwrap_or("".to_string()),
            SERVICE_NAME_LABEL,
            builder.current_service_name().unwrap_or("".to_string()),
        ));
        let pod = Api::v1Pod(self.client())
            .within(&namespace)
            .list(&p)?
            .items
            .into_iter()
            .next()
            .expect("At least one pod should be available");

        Ok(builder
            .id(pod.metadata.name)
            .started_at(Utc::now()) // TODO: https://docs.rs/k8s-openapi/0.6.0/k8s_openapi/apimachinery/pkg/apis/meta/v1/struct.ObjectMeta.html#structfield.creation_timestamp
            .build()?)
    }

    fn get_services_of_app(
        &self,
        app_name: &String,
    ) -> Result<Vec<Service>, KubernetesInfrastructureError> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}",
            APP_NAME_LABEL, app_name, SERVICE_NAME_LABEL
        ));

        let mut services = Vec::new();
        for deployment in Api::v1Deployment(self.client()).list(&p)?.items.into_iter() {
            let service = match self.create_service_from(deployment) {
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

    fn post_service_and_custom_resource_definitions(
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
        Api::v1Service(self.client()).within(&app_name).create(
            &PostParams::default(),
            service_payload(app_name, service_config).into_bytes(),
        )?;

        let request = RawApi::customResource("ingressroutes")
            .group("traefik.containo.us")
            .version("v1alpha1")
            .within(&app_name)
            .create(
                &PostParams::default(),
                ingress_route_payload(app_name, service_config).into_bytes(),
            )?;

        self.client().request_text(request)?;

        let request = RawApi::customResource("middlewares")
            .group("traefik.containo.us")
            .version("v1alpha1")
            .within(&app_name)
            .create(
                &PostParams::default(),
                middleware_payload(app_name, service_config).into_bytes(),
            )?;

        self.client().request_text(request)?;

        Ok(())
    }
}

#[async_trait]
impl Infrastructure for KubernetesInfrastructure {
    async fn get_services(&self) -> Result<MultiMap<String, Service>, Error> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!("{},{}", APP_NAME_LABEL, SERVICE_NAME_LABEL));

        let mut apps = MultiMap::new();
        for deployment in Api::v1Deployment(self.client()).list(&p)?.items.into_iter() {
            let service = match self.create_service_from(deployment) {
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
        _container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, Error> {
        match Api::v1Namespace(self.client()).create(
            &PostParams::default(),
            namespace_payload(app_name).into_bytes(),
        ) {
            Ok(result) => {
                debug!("Successfully created namespace {}", result.metadata.name);
                Ok(())
            }
            Err(e) => match e.api_error() {
                Some(api_error) if api_error.code == 409 => {
                    debug!("Namespace {} already existed.", app_name);
                    Ok(())
                }
                _ => {
                    error!("Cannot deploy namespace: {}", e);
                    Err(e)
                }
            },
        }?;

        for service_config in configs.iter() {
            if let Some(volumes) = service_config.volumes() {
                match Api::v1Secret(self.client()).within(&app_name).create(
                    &PostParams::default(),
                    secrets_payload(app_name, service_config, volumes).into_bytes(),
                ) {
                    Ok(result) => {
                        debug!("Successfully deployed {}", result.metadata.name);
                        Ok(())
                    }
                    Err(e) => match e.api_error() {
                        Some(api_error) if api_error.code == 409 => {
                            Api::v1Secret(self.client()).within(&app_name).patch(
                                &format!("{}-{}-secret", app_name, service_config.service_name()),
                                &PatchParams::default(),
                                secrets_payload(app_name, service_config, volumes).into_bytes(),
                            )?;
                            Ok(())
                        }
                        _ => {
                            error!("Cannot deploy secret: {}", e);
                            Err(e)
                        }
                    },
                }?;
            }

            match Api::v1Deployment(self.client()).within(&app_name).create(
                &PostParams::default(),
                deployment_payload(app_name, service_config).into_bytes(),
            ) {
                Ok(result) => {
                    debug!("Successfully deployed {}", result.metadata.name);
                    self.post_service_and_custom_resource_definitions(app_name, service_config)?;
                    Ok(())
                }
                Err(e) => match e.api_error() {
                    Some(api_error) if api_error.code == 409 => {
                        Api::v1Deployment(self.client()).within(&app_name).patch(
                            &format!("{}-{}-deployment", app_name, service_config.service_name()),
                            &PatchParams::default(),
                            deployment_payload(app_name, service_config).into_bytes(),
                        )?;
                        Ok(())
                    }
                    _ => {
                        error!("Cannot deploy service: {}", e);
                        Err(e)
                    }
                },
            }?
        }

        Ok(self.get_services_of_app(app_name)?)
    }

    async fn stop_services(&self, app_name: &String) -> Result<Vec<Service>, Error> {
        let services = self.get_services_of_app(app_name)?;

        for service in &services {
            Api::v1Deployment(self.client())
                .within(&service.app_name())
                .delete(
                    &format!("{}-{}-deployment", app_name, service.service_name()),
                    &DeleteParams::default(),
                )?;
            Api::v1Service(self.client())
                .within(&service.app_name())
                .delete(service.service_name(), &DeleteParams::default())?;

            let request = RawApi::customResource("ingressroutes")
                .group("traefik.containo.us")
                .version("v1alpha1")
                .within(&service.app_name())
                .delete(
                    &format!("{}-{}-ingress-route", app_name, service.service_name()),
                    &DeleteParams::default(),
                )?;

            self.client().request_text(request)?;

            let request = RawApi::customResource("middlewares")
                .group("traefik.containo.us")
                .version("v1alpha1")
                .within(&service.app_name())
                .delete(
                    &format!("{}-{}-middleware", app_name, service.service_name()),
                    &DeleteParams::default(),
                )?;

            self.client().request_text(request)?;
        }

        Api::v1Namespace(self.client()).delete(app_name, &DeleteParams::default())?;

        Ok(services)
    }

    async fn get_configs_of_app(&self, _app_name: &String) -> Result<Vec<ServiceConfig>, Error> {
        Ok(vec![])
    }

    async fn get_logs(
        &self,
        app_name: &String,
        service_name: &String,
        _from: &Option<DateTime<FixedOffset>>,
        _limit: usize,
    ) -> Result<Option<Vec<(DateTime<FixedOffset>, String)>>, Error> {
        let mut p = ListParams::default();
        p.label_selector = Some(format!(
            "{}={},{}={}",
            APP_NAME_LABEL, app_name, SERVICE_NAME_LABEL, service_name
        ));

        let service = match Api::v1Deployment(self.client())
            .list(&p)?
            .items
            .into_iter()
            .next()
            .map(|deployment| self.create_service_from(deployment))
        {
            None => return Ok(None),
            Some(service) => service?,
        };

        let p = LogParams::default();
        // TODO:  RBAC issue when using timestamps... seems to be an invalid query string serialization
        // p.pretty = true;
        // p.timestamps = true;

        let pods = Api::v1Pod(self.client()).within(&app_name);
        let logs = pods.log(service.id(), &p)?;

        let offset = FixedOffset::east(0);
        Ok(Some(
            logs.lines()
                .into_iter()
                .map(|line| (Utc::now().with_timezone(&offset), line.to_string() + "\n"))
                .collect(),
        ))
    }

    async fn change_status(
        &self,
        _app_name: &String,
        _service_name: &String,
        _status: ServiceStatus,
    ) -> Result<Option<Service>, Error> {
        // TODO: https://stackoverflow.com/a/54822866
        unimplemented!()
    }
}

impl TryFrom<Deployment> for ServiceBuilder {
    type Error = KubernetesInfrastructureError;

    fn try_from(deployment: Deployment) -> Result<Self, Self::Error> {
        let labels = deployment.metadata.labels;

        let mut builder = match labels.get(APP_NAME_LABEL) {
            Some(app_name) => ServiceBuilder::new().app_name(app_name.clone()),
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

        Ok(builder)
    }
}

impl From<kube::Error> for KubernetesInfrastructureError {
    fn from(err: kube::Error) -> Self {
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
