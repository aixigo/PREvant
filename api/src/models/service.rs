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

use crate::models::{web_host_meta::WebHostMeta, AppName, ServiceConfig};
use chrono::{DateTime, Utc};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde::Deserialize;
use std::fmt::Display;
use std::str::FromStr;
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Services(Vec<Service>);

impl Services {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn into_iter(self) -> impl Iterator<Item = Service> {
        self.0.into_iter()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Service> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Serialize for Services {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.len()))?;

        for service in self.iter() {
            seq.serialize_element(service)?;
        }

        serde::ser::SerializeSeq::end(seq)
    }
}

impl From<Vec<Service>> for Services {
    fn from(services: Vec<Service>) -> Self {
        if services.is_empty() {
            return Self::empty();
        }

        let mut services = services;
        services.sort_by_key(|service| service.config.service_name().clone());
        Self(services)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Service {
    /// An unique identifier of the service, e.g. the container id
    pub id: String,
    pub state: State,
    pub config: ServiceConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub status: ServiceStatus,
    #[serde(skip)]
    pub started_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ServiceStatus {
    Running,
    Paused,
}

impl Service {
    pub fn id(&self) -> &String {
        &self.id
    }

    pub fn service_name(&self) -> &String {
        self.config.service_name()
    }

    pub fn container_type(&self) -> &ContainerType {
        self.config.container_type()
    }

    pub fn started_at(&self) -> &Option<DateTime<Utc>> {
        &self.state.started_at
    }

    pub fn status(&self) -> &ServiceStatus {
        &self.state.status
    }
}

impl Serialize for Service {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_map(Some(3))?;
        s.serialize_entry("name", self.service_name())?;
        s.serialize_entry("type", self.config.container_type())?;
        s.serialize_entry("state", &self.state)?;

        s.end()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServiceWithHostMeta {
    /// An unique identifier of the service, e.g. the container id
    id: String,
    pub service_url: Option<Url>,
    pub web_host_meta: WebHostMeta,
    pub state: State,
    pub config: ServiceConfig,
}

impl ServiceWithHostMeta {
    pub fn from_service_and_web_host_meta(
        service: Service,
        web_host_meta: WebHostMeta,
        base_url: Url,
        app_name: &AppName,
    ) -> Self {
        let service_url = if !web_host_meta.is_valid() {
            None
        } else {
            let mut base_url = base_url;
            base_url.path_segments_mut().expect("").extend([
                app_name,
                service.config.service_name(),
                &String::from(""),
            ]);
            Some(base_url)
        };

        Self {
            id: service.id,
            service_url,
            web_host_meta,
            state: service.state,
            config: service.config,
        }
    }
}

impl Serialize for ServiceWithHostMeta {
    fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Service<'a> {
            name: &'a String,
            #[serde(skip_serializing_if = "Option::is_none")]
            url: &'a Option<Url>,
            #[serde(rename = "type")]
            service_type: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            version: Option<Version>,
            #[serde(skip_serializing_if = "Option::is_none")]
            open_api_url: Option<&'a Url>,
            #[serde(skip_serializing_if = "Option::is_none")]
            async_api_url: Option<&'a Url>,
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

        let open_api_url = self.web_host_meta.openapi();
        let version = if !self.web_host_meta.is_empty() {
            Some(Version {
                git_commit: self.web_host_meta.commit(),
                software_version: self.web_host_meta.version(),
                date_modified: self.web_host_meta.date_modified(),
            })
        } else {
            None
        };

        let s = Service {
            name: self.config.service_name(),
            url: &self.service_url,
            service_type: self.config.container_type().to_string(),
            version,
            open_api_url,
            async_api_url: self.web_host_meta.asyncapi(),
            state: &self.state,
        };

        s.serialize(serializer)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServicesWithHostMeta(Vec<ServiceWithHostMeta>);

impl Serialize for ServicesWithHostMeta {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;

        for service in self.0.iter() {
            seq.serialize_element(service)?;
        }

        serde::ser::SerializeSeq::end(seq)
    }
}

impl From<Vec<ServiceWithHostMeta>> for ServicesWithHostMeta {
    fn from(services: Vec<ServiceWithHostMeta>) -> Self {
        let mut services = services;
        services.sort_by_key(|service| service.config.service_name().clone());
        Self(services)
    }
}

#[derive(Debug, Default, Deserialize, Clone, Eq, Hash, PartialEq, Serialize)]
pub enum ContainerType {
    #[serde(rename = "instance")]
    #[default]
    Instance,
    #[serde(rename = "replica")]
    Replica,
    #[serde(rename = "app-companion")]
    ApplicationCompanion,
    #[serde(rename = "service-companion")]
    ServiceCompanion,
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
            ContainerType::Instance => write!(f, "instance"),
            ContainerType::Replica => write!(f, "replica"),
            ContainerType::ApplicationCompanion => write!(f, "app-companion"),
            ContainerType::ServiceCompanion => write!(f, "service-companion"),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ServiceError {
    #[error("Invalid service type label: {label}")]
    InvalidServiceType { label: String },
    #[error("Invalid image: {invalid_string}")]
    InvalidImageString { invalid_string: String },
}

#[cfg(test)]
mod tests {
    use assert_json_diff::assert_json_eq;

    use super::*;

    #[test]
    fn serialize_service() {
        assert_json_eq!(
            serde_json::json!({
                "name": "mariadb",
                "type": "instance",
                "state": {
                    "status": "running"
                }
            }),
            serde_json::to_value(Service {
                id: String::from("some id"),
                state: State {
                    status: ServiceStatus::Running,
                    started_at: Some(Utc::now()),
                },
                config: crate::sc!("mariadb", "mariadb:latest")
            })
            .unwrap()
        );
    }

    #[test]
    fn serialize_services() {
        assert_json_eq!(
            serde_json::json!([{
                "name": "mariadb",
                "type": "instance",
                "state": {
                    "status": "running"
                }
            }, {
                "name": "postgres",
                "type": "instance",
                "state": {
                    "status": "running"
                }
            }]),
            serde_json::to_value(Services::from(vec![
                Service {
                    id: String::from("some id"),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: Some(Utc::now()),
                    },
                    config: crate::sc!("postgres", "postgres:latest")
                },
                Service {
                    id: String::from("some id"),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: Some(Utc::now()),
                    },
                    config: crate::sc!("mariadb", "mariadb:latest")
                }
            ]))
            .unwrap()
        );
    }

    #[test]
    fn serialize_services_with_web_host_meta() {
        let base_url = Url::from_str("http://prevant.example.com").unwrap();
        let app_name = AppName::master();

        assert_json_eq!(
            serde_json::json!([{
                "name": "mariadb",
                "type": "instance",
                "state": {
                    "status": "running"
                },
                "url": "http://prevant.example.com/master/mariadb/",
                "version": {
                    "softwareVersion": "1.2.3"
                }
            }, {
                "name": "postgres",
                "type": "instance",
                "state": {
                    "status": "running"
                }
            }]),
            serde_json::to_value(ServicesWithHostMeta::from(vec![
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("some id"),
                        state: State {
                            status: ServiceStatus::Running,
                            started_at: Some(Utc::now()),
                        },
                        config: crate::sc!("postgres", "postgres:latest")
                    },
                    WebHostMeta::invalid(),
                    base_url.clone(),
                    &app_name
                ),
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("some id"),
                        state: State {
                            status: ServiceStatus::Running,
                            started_at: Some(Utc::now()),
                        },
                        config: crate::sc!("mariadb", "mariadb:latest")
                    },
                    WebHostMeta::with_version(String::from("1.2.3")),
                    base_url,
                    &app_name
                )
            ]))
            .unwrap()
        );
    }
}
