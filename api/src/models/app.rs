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

use crate::models::user_defined_parameters::UserDefinedParameters;
use crate::models::{web_host_meta::WebHostMeta, AppName, ServiceConfig};
use chrono::{DateTime, Utc};
use openidconnect::{IssuerUrl, SubjectIdentifier};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::str::FromStr;
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct Owner {
    pub sub: SubjectIdentifier,
    pub iss: IssuerUrl,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Owner {
    pub fn normalize(owners: HashSet<Self>) -> HashSet<Self> {
        let mut map = HashMap::<(SubjectIdentifier, IssuerUrl), Option<String>>::new();

        for owner in owners.into_iter() {
            let Owner { sub, iss, mut name } = owner;

            map.entry((sub, iss))
                .and_modify(|existing_name| {
                    *existing_name = match (existing_name.take(), name.take()) {
                        (None, None) => None,
                        (None, Some(name)) => Some(name),
                        (Some(name), None) => Some(name),
                        (Some(name_1), Some(name_2)) => {
                            // names with spaces will be prioritize because they are most likely
                            // the real name.
                            match (name_1.contains(" "), name_2.contains(" ")) {
                                (true, false) => Some(name_1),
                                (false, true) => Some(name_2),
                                _ => {
                                    if name_1.len() > name_2.len() {
                                        Some(name_1)
                                    } else {
                                        Some(name_2)
                                    }
                                }
                            }
                        }
                    };
                })
                .or_insert(name);
        }

        map.into_iter()
            .map(|((sub, iss), name)| Owner { sub, iss, name })
            .collect::<HashSet<_>>()
    }
}

/// Data structure for holding information about the application. For example, which services are
/// deployed and who created them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct App {
    services: Vec<Service>,
    owners: HashSet<Owner>,
    user_defined_parameters: Option<UserDefinedParameters>,
}

impl App {
    pub fn new(
        services: Vec<Service>,
        owners: HashSet<Owner>,
        user_defined_payload: Option<UserDefinedParameters>,
    ) -> Self {
        if services.is_empty() {
            return Self::empty();
        }

        let mut services = services;
        services.sort_by(|service1, service2| {
            service1
                .config
                .service_name()
                .cmp(service2.config.service_name())
        });

        Self {
            services,
            owners: Owner::normalize(owners),
            user_defined_parameters: user_defined_payload,
        }
    }

    pub fn empty() -> Self {
        Self {
            services: Vec::new(),
            owners: HashSet::new(),
            user_defined_parameters: None,
        }
    }

    pub fn into_services(self) -> Vec<Service> {
        self.services
    }

    pub fn user_defined_parameters(&self) -> &Option<UserDefinedParameters> {
        &self.user_defined_parameters
    }

    pub fn into_services_and_user_defined_parameters(
        self,
    ) -> (Vec<Service>, Option<UserDefinedParameters>) {
        (self.services, self.user_defined_parameters)
    }

    pub fn into_services_and_owners(self) -> (Vec<Service>, HashSet<Owner>) {
        (self.services, self.owners)
    }

    pub fn services(&self) -> &[Service] {
        &self.services
    }

    pub fn owners(&self) -> &HashSet<Owner> {
        &self.owners
    }

    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
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
pub struct AppWithHostMeta {
    services: Vec<ServiceWithHostMeta>,
    owners: HashSet<Owner>,
}

impl AppWithHostMeta {
    pub fn new(services: Vec<ServiceWithHostMeta>, owners: HashSet<Owner>) -> Self {
        let mut services = services;
        services.sort_by(|service1, service2| {
            service1
                .config
                .service_name()
                .cmp(service2.config.service_name())
        });
        Self {
            services,
            owners: Owner::normalize(owners),
        }
    }

    pub fn services(&self) -> &[ServiceWithHostMeta] {
        &self.services
    }

    pub fn owners(&self) -> &HashSet<Owner> {
        &self.owners
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
    use super::*;
    use crate::sc;
    use assert_json_diff::assert_json_eq;

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
                state: crate::models::State {
                    status: ServiceStatus::Running,
                    started_at: Some(Utc::now()),
                },
                config: crate::sc!("mariadb", "mariadb:latest")
            })
            .unwrap()
        );
    }

    #[test]
    fn app_eq_with_different_service_order_construction() {
        let app1 = App::new(
            vec![
                Service {
                    id: String::from("b1"),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: None,
                    },
                    config: sc!("b"),
                },
                Service {
                    id: String::from("a1"),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: None,
                    },
                    config: sc!("a"),
                },
            ],
            HashSet::new(),
            None,
        );
        let app2 = App::new(
            vec![
                Service {
                    id: String::from("a1"),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: None,
                    },
                    config: sc!("a"),
                },
                Service {
                    id: String::from("b1"),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: None,
                    },
                    config: sc!("b"),
                },
            ],
            HashSet::new(),
            None,
        );

        assert_eq!(app1, app2);
    }

    #[test]
    fn app_with_host_meta_eq_with_different_service_order_construction() {
        let url = Url::parse("http://prevant.examle.com").unwrap();
        let app_name = AppName::master();
        let app1 = AppWithHostMeta::new(
            vec![
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("b1"),
                        state: State {
                            status: ServiceStatus::Running,
                            started_at: None,
                        },
                        config: sc!("b"),
                    },
                    WebHostMeta::empty(),
                    url.clone(),
                    &app_name,
                ),
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("a1"),
                        state: State {
                            status: ServiceStatus::Running,
                            started_at: None,
                        },
                        config: sc!("a"),
                    },
                    WebHostMeta::empty(),
                    url.clone(),
                    &app_name,
                ),
            ],
            HashSet::new(),
        );
        let app2 = AppWithHostMeta::new(
            vec![
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("a1"),
                        state: State {
                            status: ServiceStatus::Running,
                            started_at: None,
                        },
                        config: sc!("a"),
                    },
                    WebHostMeta::empty(),
                    url.clone(),
                    &app_name,
                ),
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("b1"),
                        state: State {
                            status: ServiceStatus::Running,
                            started_at: None,
                        },
                        config: sc!("b"),
                    },
                    WebHostMeta::empty(),
                    url,
                    &app_name,
                ),
            ],
            HashSet::new(),
        );

        assert_eq!(app1, app2);
    }

    #[test]
    fn app_without_host_meta_normalizes_owners() {
        let app = App::new(
            vec![Service {
                id: String::from("a1"),
                state: State {
                    status: ServiceStatus::Running,
                    started_at: None,
                },
                config: sc!("a"),
            }],
            HashSet::from([
                Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: Some(String::from("user_login")),
                },
                Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: Some(String::from("Some Person")),
                },
                Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: None,
                },
            ]),
            None,
        );

        assert_eq!(
            app.owners,
            HashSet::from([Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            }]),
        )
    }
    #[test]
    fn app_with_host_meta_normalizes_owners() {
        let url = Url::parse("http://prevant.examle.com").unwrap();
        let app_name = AppName::master();
        let app = AppWithHostMeta::new(
            vec![ServiceWithHostMeta::from_service_and_web_host_meta(
                Service {
                    id: String::from("a1"),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: None,
                    },
                    config: sc!("a"),
                },
                WebHostMeta::empty(),
                url.clone(),
                &app_name,
            )],
            HashSet::from([
                Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: Some(String::from("user_login")),
                },
                Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: Some(String::from("Some Person")),
                },
                Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: None,
                },
            ]),
        );

        assert_eq!(
            app.owners,
            HashSet::from([Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            }]),
        )
    }

    #[test]
    fn merge_owners_with_same_sub_issuer() {
        let owners = HashSet::from([
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("user_login")),
            },
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: None,
            },
        ]);

        let owners = Owner::normalize(owners);

        assert_eq!(
            owners,
            HashSet::from([Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },])
        )
    }
}
