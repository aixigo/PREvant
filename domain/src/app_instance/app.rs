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

use crate::{
    AppName, Owner,
    app_blueprints::{ServiceConfig, UserDefinedParameters},
    app_instance::{ContainerType, WebHostMeta},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{collections::HashSet, marker::PhantomData};
use url::Url;

/// Data structure for holding information about the application. For example, which services are
/// deployed and who created them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct App {
    pub services: Vec<Service>,
    pub owners: HashSet<Owner>,
    pub user_defined_parameters: Option<UserDefinedParameters>,
    pub created_at: Option<DateTime<Utc>>,
    phantom_data: PhantomData<()>,
}

impl App {
    pub fn new(
        services: Vec<Service>,
        owners: HashSet<Owner>,
        user_defined_payload: Option<UserDefinedParameters>,
        created_at: Option<DateTime<Utc>>,
    ) -> Self {
        let mut services = services;
        services.sort_by(|service1, service2| {
            service1
                .blueprint_config
                .service_name
                .cmp(&service2.blueprint_config.service_name)
        });

        Self {
            services,
            owners: Owner::normalize(owners),
            user_defined_parameters: user_defined_payload,
            created_at,
            phantom_data: PhantomData,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Service {
    /// An unique identifier of the service, e.g. the Docker container id
    pub id: String,
    pub status: ServiceStatus,
    pub service_type: ContainerType,
    /// The [`ServiceConfig`] from which the deployed service has been derived.
    pub blueprint_config: ServiceConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceStatus {
    Running { started_at: DateTime<Utc> },
    Paused,
}

impl Service {
    pub fn id(&self) -> &String {
        &self.id
    }

    pub fn service_name(&self) -> &String {
        &self.blueprint_config.service_name
    }

    pub fn started_at(&self) -> Option<DateTime<Utc>> {
        match self.status {
            ServiceStatus::Running { started_at } => Some(started_at),
            ServiceStatus::Paused => None,
        }
    }
}

impl Serialize for Service {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        use serde::ser::SerializeMap;

        #[derive(Serialize)]
        struct State {
            status: &'static str,
        }

        let mut s = serializer.serialize_map(Some(3))?;
        s.serialize_entry("name", &self.blueprint_config.service_name)?;
        s.serialize_entry("type", &self.service_type)?;
        s.serialize_entry(
            "state",
            &State {
                status: match self.status {
                    ServiceStatus::Running { .. } => "running",
                    ServiceStatus::Paused => "paused",
                },
            },
        )?;

        s.end()
    }
}

// TODO: instead of two different structs we should probably use an enum.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceWithHostMeta {
    /// An unique identifier of the service, e.g. the container id
    id: String,
    pub service_url: Option<Url>,
    pub web_host_meta: WebHostMeta,
    pub status: ServiceStatus,
    /// The [`ServiceConfig`] from which the deployed service has been derived.
    pub blueprint_config: ServiceConfig,
    pub service_type: ContainerType,
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
                &service.blueprint_config.service_name,
                &String::from(""),
            ]);
            Some(base_url)
        };

        Self {
            id: service.id,
            service_url,
            web_host_meta,
            status: service.status,
            blueprint_config: service.blueprint_config,
            service_type: service.service_type,
        }
    }
}

impl Serialize for ServiceWithHostMeta {
    fn serialize<S>(
        &self,
        serializer: S,
    ) -> Result<<S as serde::ser::Serializer>::Ok, <S as serde::ser::Serializer>::Error>
    where
        S: serde::ser::Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ServiceState<'a> {
            status: &'a str,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Service<'a> {
            name: &'a String,
            #[serde(skip_serializing_if = "Option::is_none")]
            url: &'a Option<Url>,
            #[serde(rename = "type")]
            service_type: &'a ContainerType,
            #[serde(skip_serializing_if = "Option::is_none")]
            version: Option<Version>,
            #[serde(skip_serializing_if = "Option::is_none")]
            open_api_url: Option<&'a Url>,
            #[serde(skip_serializing_if = "Option::is_none")]
            async_api_url: Option<&'a Url>,
            state: ServiceState<'a>,
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
            name: &self.blueprint_config.service_name,
            url: &self.service_url,
            service_type: &self.service_type,
            version,
            open_api_url,
            async_api_url: self.web_host_meta.asyncapi(),
            state: ServiceState {
                status: match self.status {
                    ServiceStatus::Running { started_at: _ } => "running",
                    ServiceStatus::Paused => "paused",
                },
            },
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
                .blueprint_config
                .service_name
                .cmp(&service2.blueprint_config.service_name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use assert_json_diff::assert_json_eq;
    use chrono::TimeDelta;
    use openidconnect::{IssuerUrl, SubjectIdentifier};

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
                service_type: ContainerType::Instance,
                status: ServiceStatus::Running {
                    started_at: Utc::now(),
                },
                blueprint_config: blueprint_service!("mariadb", "mariadb:latest")
            })
            .unwrap()
        );
    }

    #[test]
    fn app_eq_with_different_service_order_construction() {
        let now_b1 = Utc::now();
        let now_a1 = now_b1 + TimeDelta::minutes(3);
        let app1 = App::new(
            vec![
                Service {
                    id: String::from("b1"),
                    service_type: ContainerType::Instance,
                    status: ServiceStatus::Running { started_at: now_b1 },
                    blueprint_config: blueprint_service!("b"),
                },
                Service {
                    id: String::from("a1"),
                    service_type: ContainerType::Instance,
                    status: ServiceStatus::Running { started_at: now_a1 },
                    blueprint_config: blueprint_service!("a"),
                },
            ],
            HashSet::new(),
            None,
            None,
        );
        let app2 = App::new(
            vec![
                Service {
                    id: String::from("a1"),
                    service_type: ContainerType::Instance,
                    status: ServiceStatus::Running { started_at: now_a1 },
                    blueprint_config: blueprint_service!("a"),
                },
                Service {
                    id: String::from("b1"),
                    service_type: ContainerType::Instance,
                    status: ServiceStatus::Running { started_at: now_b1 },
                    blueprint_config: blueprint_service!("b"),
                },
            ],
            HashSet::new(),
            None,
            None,
        );

        assert_eq!(app1, app2);
    }

    #[test]
    fn app_with_host_meta_eq_with_different_service_order_construction() {
        let now_b1 = Utc::now();
        let now_a1 = now_b1 + TimeDelta::minutes(3);
        let url = Url::parse("http://prevant.examle.com").unwrap();
        let app_name = AppName::master();
        let app1 = AppWithHostMeta::new(
            vec![
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("b1"),
                        service_type: ContainerType::Instance,
                        status: ServiceStatus::Running { started_at: now_b1 },
                        blueprint_config: blueprint_service!("b"),
                    },
                    WebHostMeta::empty(),
                    url.clone(),
                    &app_name,
                ),
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("a1"),
                        service_type: ContainerType::Instance,
                        status: ServiceStatus::Running { started_at: now_a1 },
                        blueprint_config: blueprint_service!("a"),
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
                        service_type: ContainerType::Instance,
                        status: ServiceStatus::Running { started_at: now_a1 },
                        blueprint_config: blueprint_service!("a"),
                    },
                    WebHostMeta::empty(),
                    url.clone(),
                    &app_name,
                ),
                ServiceWithHostMeta::from_service_and_web_host_meta(
                    Service {
                        id: String::from("b1"),
                        service_type: ContainerType::Instance,
                        status: ServiceStatus::Running { started_at: now_b1 },
                        blueprint_config: blueprint_service!("b"),
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
                service_type: ContainerType::Instance,
                status: ServiceStatus::Running {
                    started_at: Utc::now(),
                },
                blueprint_config: blueprint_service!("a"),
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
                    service_type: ContainerType::Instance,
                    status: ServiceStatus::Running {
                        started_at: Utc::now(),
                    },
                    blueprint_config: blueprint_service!("a"),
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
}
