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

use crate::models::{web_host_meta::WebHostMeta, Image, ServiceConfig};
use chrono::{DateTime, Utc};
use serde::ser::{Serialize, Serializer};
use serde::Deserialize;
use std::fmt::Display;
use std::net::IpAddr;
use std::str::FromStr;
use url::Url;

#[derive(Clone, Debug)]
pub struct Service {
    /// An unique identifier of the service, e.g. the container id
    id: String,
    app_name: String,
    base_url: Option<Url>,
    endpoint: Option<ServiceEndpoint>,
    web_host_meta: Option<WebHostMeta>,
    state: State,
    config: ServiceConfig,
}

#[derive(Clone, Debug)]
struct ServiceEndpoint {
    internal_addr: IpAddr,
    exposed_port: u16,
}

impl ServiceEndpoint {
    fn to_url(&self) -> Url {
        Url::parse(&format!(
            "http://{}:{}/",
            self.internal_addr, self.exposed_port
        ))
        .unwrap()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    status: ServiceStatus,
    #[serde(skip)]
    started_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ServiceStatus {
    Running,
    Paused,
}

impl Service {
    pub fn app_name(&self) -> &String {
        &self.app_name
    }

    fn service_url(&self) -> Option<Url> {
        self.base_url.clone().map(|url| {
            url.join(&format!("/{}/{}/", &self.app_name, self.service_name()))
                .unwrap()
        })
    }

    pub fn id(&self) -> &String {
        &self.id
    }

    pub fn service_name(&self) -> &String {
        &self.config.service_name()
    }

    pub fn container_type(&self) -> &ContainerType {
        &self.config.container_type()
    }

    pub fn config(&self) -> &ServiceConfig {
        &self.config
    }

    pub fn port(&self) -> Option<u16> {
        match &self.endpoint {
            None => None,
            Some(endpoint) => Some(endpoint.exposed_port),
        }
    }

    pub fn endpoint_url(&self) -> Option<Url> {
        match &self.endpoint {
            None => None,
            Some(endpoint) => Some(endpoint.to_url()),
        }
    }

    pub fn started_at(&self) -> &DateTime<Utc> {
        &self.state.started_at
    }

    pub fn status(&self) -> &ServiceStatus {
        &self.state.status
    }

    pub fn image(&self) -> &Image {
        self.config.image()
    }
}

impl Serialize for Service {
    fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Service<'a> {
            name: &'a String,
            #[serde(skip_serializing_if = "Option::is_none")]
            url: Option<String>,
            #[serde(rename = "type")]
            service_type: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            version: Option<Version>,
            #[serde(skip_serializing_if = "Option::is_none")]
            open_api_url: Option<String>,
            state: &'a State,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Version {
            #[serde(skip_serializing_if = "Option::is_none")]
            git_commit: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            software_version: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            date_modified: Option<DateTime<Utc>>,
        }

        let open_api_url = self.web_host_meta.clone().and_then(|meta| meta.openapi());
        let version = match &self.web_host_meta {
            Some(meta) if !meta.is_empty() => Some(Version {
                git_commit: meta.commit(),
                software_version: meta.version(),
                date_modified: meta.date_modified(),
            }),
            _ => None,
        };

        let s = Service {
            name: self.service_name(),
            url: match self.web_host_meta {
                Some(ref meta) if meta.is_valid() => self.service_url().map(|url| url.to_string()),
                _ => None,
            },
            service_type: self.container_type().to_string(),
            version,
            open_api_url,
            state: &self.state,
        };

        Ok(s.serialize(serializer)?)
    }
}

#[derive(Debug)]
pub struct ServiceBuilder {
    id: Option<String>,
    app_name: Option<String>,
    config: Option<ServiceConfig>,
    status: Option<ServiceStatus>,
    started_at: Option<DateTime<Utc>>,
    base_url: Option<Url>,
    web_host_meta: Option<WebHostMeta>,
    endpoint: Option<ServiceEndpoint>,
}

impl ServiceBuilder {
    pub fn new() -> Self {
        ServiceBuilder {
            id: None,
            app_name: None,
            status: None,
            started_at: None,
            base_url: None,
            web_host_meta: None,
            endpoint: None,
            config: None,
        }
    }

    pub fn build(self) -> Result<Service, ServiceBuilderError> {
        let id = self.id.ok_or(ServiceBuilderError::MissingId)?;
        let app_name = self.app_name.ok_or(ServiceBuilderError::MissingAppName)?;
        let config = self
            .config
            .ok_or(ServiceBuilderError::MissingServiceConfiguration)?;
        let started_at = self.started_at.unwrap_or(Utc::now());

        Ok(Service {
            id,
            app_name,
            config,
            base_url: self.base_url,
            endpoint: self.endpoint,
            web_host_meta: self.web_host_meta,
            state: State {
                started_at,
                status: self.status.unwrap_or(ServiceStatus::Running),
            },
        })
    }

    pub fn id(mut self, id: String) -> Self {
        self.id = Some(id);
        self
    }

    pub fn app_name(mut self, app_name: String) -> Self {
        self.app_name = Some(app_name);
        self
    }

    pub fn current_app_name(&self) -> Option<&String> {
        self.app_name.as_ref()
    }

    pub fn current_config(&self) -> Option<&ServiceConfig> {
        self.config.as_ref()
    }

    pub fn started_at(mut self, started_at: DateTime<Utc>) -> Self {
        self.started_at = Some(started_at);
        self
    }

    pub fn service_status(mut self, service_status: ServiceStatus) -> Self {
        self.status = Some(service_status);
        self
    }

    pub fn base_url(mut self, base_url: Url) -> Self {
        self.base_url = Some(base_url);
        self
    }

    pub fn web_host_meta(mut self, web_host_meta: WebHostMeta) -> Self {
        self.web_host_meta = Some(web_host_meta);
        self
    }

    pub fn config(mut self, config: ServiceConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn endpoint(mut self, addr: IpAddr, port: u16) -> Self {
        self.endpoint = Some(ServiceEndpoint {
            internal_addr: addr,
            exposed_port: port,
        });
        self
    }
}

#[derive(Debug, Fail, PartialEq)]
pub enum ServiceBuilderError {
    #[fail(display = "An ID must be provided.")]
    MissingId,
    #[fail(display = "An app name must be provided.")]
    MissingAppName,
    #[fail(display = "A service configuration must be provided.")]
    MissingServiceConfiguration,
}

impl From<Service> for ServiceBuilder {
    fn from(service: Service) -> Self {
        ServiceBuilder {
            id: Some(service.id),
            app_name: Some(service.app_name),
            config: Some(service.config),
            status: Some(service.state.status),
            started_at: Some(service.state.started_at),
            base_url: service.base_url,
            web_host_meta: service.web_host_meta,
            endpoint: service.endpoint,
        }
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize)]
pub enum ContainerType {
    #[serde(rename = "instance")]
    Instance,
    #[serde(rename = "replica")]
    Replica,
    #[serde(rename = "app-companion")]
    ApplicationCompanion,
    #[serde(rename = "service-companion")]
    ServiceCompanion,
}

impl Default for ContainerType {
    fn default() -> ContainerType {
        ContainerType::Instance
    }
}

impl FromStr for ContainerType {
    type Err = ServiceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "replica" => Ok(ContainerType::Replica),
            "instance" => Ok(ContainerType::Instance),
            "app-companion" => Ok(ContainerType::ApplicationCompanion),
            "service-companion" => Ok(ContainerType::ServiceCompanion),
            label => Err(ServiceError::InvalidServiceType {
                label: String::from(label),
            }),
        }
    }
}

impl Display for ContainerType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            &ContainerType::Instance => write!(f, "instance"),
            &ContainerType::Replica => write!(f, "replica"),
            &ContainerType::ApplicationCompanion => write!(f, "app-companion"),
            &ContainerType::ServiceCompanion => write!(f, "service-companion"),
        }
    }
}

#[derive(Debug, Fail)]
pub enum ServiceError {
    #[fail(display = "Invalid service type label: {}", label)]
    InvalidServiceType { label: String },
    #[fail(
        display = "Service name {:?} does not match pattern ((.+)-.+).",
        invalid_name
    )]
    InvalidServiceName { invalid_name: String },
    #[fail(display = "Invalid image: {}", invalid_string)]
    InvalidImageString { invalid_string: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sc;

    #[test]
    fn should_build_service() {
        let started_at = Utc::now();

        let service = ServiceBuilder::new()
            .id("some-random-id".to_string())
            .app_name("master".to_string())
            .config(sc!("nginx", "nginx"))
            .started_at(started_at)
            .build()
            .unwrap();

        assert_eq!(service.id(), "some-random-id");
        assert_eq!(service.app_name(), "master");
        assert_eq!(service.config(), &sc!("nginx", "nginx"));
        assert_eq!(service.started_at(), &started_at);
        assert_eq!(service.state.status, ServiceStatus::Running);
    }

    #[test]
    fn should_build_service_with_service_status() {
        let service = ServiceBuilder::new()
            .id("some-random-id".to_string())
            .app_name("master".to_string())
            .config(sc!("nginx", "nginx"))
            .started_at(Utc::now())
            .service_status(ServiceStatus::Paused)
            .build()
            .unwrap();

        assert_eq!(service.state.status, ServiceStatus::Paused);
    }

    #[test]
    fn should_build_service_with_base_url() {
        let url = Url::parse("http://example.com").unwrap();

        let service = ServiceBuilder::new()
            .id("some-random-id".to_string())
            .app_name("master".to_string())
            .config(sc!("nginx", "nginx"))
            .started_at(Utc::now())
            .base_url(url.clone())
            .build()
            .unwrap();

        assert_eq!(
            service.service_url(),
            Some(url.join("/master/nginx/").unwrap())
        );
    }

    #[test]
    fn should_build_service_with_web_host_meta() {
        let meta = WebHostMeta::empty();

        let service = ServiceBuilder::new()
            .id("some-random-id".to_string())
            .app_name("master".to_string())
            .config(sc!("nginx", "nginx"))
            .started_at(Utc::now())
            .web_host_meta(meta.clone())
            .build()
            .unwrap();

        assert_eq!(service.web_host_meta, Some(meta));
    }

    #[test]
    fn should_not_build_service_missing_id() {
        let err = ServiceBuilder::new().build().unwrap_err();
        assert_eq!(err, ServiceBuilderError::MissingId);
    }

    #[test]
    fn should_not_build_service_missing_app_name() {
        let err = ServiceBuilder::new()
            .id("some-container-id".to_string())
            .build()
            .unwrap_err();

        assert_eq!(err, ServiceBuilderError::MissingAppName);
    }

    #[test]
    fn should_not_build_service_missing_config() {
        let err = ServiceBuilder::new()
            .id("some-container-id".to_string())
            .app_name("master".to_string())
            .build()
            .unwrap_err();

        assert_eq!(err, ServiceBuilderError::MissingServiceConfiguration);
    }
}
