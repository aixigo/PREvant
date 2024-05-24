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

use crate::config::ContainerConfig;
use crate::deployment::deployment_unit::DeployableService;
use crate::deployment::DeploymentUnit;
use crate::infrastructure::Infrastructure;
use crate::models::service::{Service, ServiceStatus, Services, State};
use crate::models::{AppName, ServiceConfig};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use futures::stream::{self, BoxStream};
use multimap::MultiMap;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::TraefikIngressRoute;

#[cfg(test)]
#[derive(Clone)]
pub struct DummyInfrastructure {
    delay: Option<Duration>,
    services: Arc<Mutex<MultiMap<AppName, DeployableService>>>,
    base_ingress_route: Option<TraefikIngressRoute>,
}

#[cfg(test)]
impl DummyInfrastructure {
    pub fn new() -> Self {
        Self {
            delay: None,
            services: Arc::new(Mutex::new(MultiMap::new())),
            base_ingress_route: None,
        }
    }

    pub fn with_delay(delay: Duration) -> Self {
        Self {
            delay: Some(delay),
            services: Arc::new(Mutex::new(MultiMap::new())),
            base_ingress_route: None,
        }
    }

    pub fn with_base_route(base_ingress_route: TraefikIngressRoute) -> Self {
        Self {
            delay: None,
            services: Arc::new(Mutex::new(MultiMap::new())),
            base_ingress_route: Some(base_ingress_route),
        }
    }

    pub fn services(&self) -> Vec<DeployableService> {
        self.services
            .lock()
            .unwrap()
            .iter_all()
            .flat_map(|(_, v)| v.iter().cloned())
            .collect::<Vec<_>>()
    }
}

#[cfg(test)]
impl DummyInfrastructure {
    async fn delay_if_configured(&self) {
        if let Some(delay) = &self.delay {
            tokio::time::sleep(*delay).await;
        }
    }
}

#[cfg(test)]
#[async_trait]
impl Infrastructure for DummyInfrastructure {
    async fn fetch_services(&self) -> Result<HashMap<AppName, Services>> {
        let mut s = HashMap::new();

        let services = self.services.lock().unwrap();
        for (app, configs) in services.iter_all() {
            let mut services = Vec::with_capacity(configs.len());
            for config in configs {
                let service = Service {
                    id: config.service_name().clone(),
                    config: ServiceConfig::clone(config),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: DateTime::parse_from_rfc3339("2019-07-18T07:30:00.000000000Z")
                            .unwrap()
                            .with_timezone(&Utc),
                    },
                };

                services.push(service);
            }

            s.insert(AppName::from_str(app).unwrap(), Services::from(services));
        }

        Ok(s)
    }

    async fn deploy_services(
        &self,
        _status_id: &str,
        deployment_unit: &DeploymentUnit,
        _container_config: &ContainerConfig,
    ) -> Result<Services> {
        self.delay_if_configured().await;

        let mut services = self.services.lock().unwrap();
        let app_name = deployment_unit.app_name();
        let deployable_services = deployment_unit.services();
        if let Some(running_services) = services.get_vec_mut(&app_name) {
            let service_names = deployable_services
                .iter()
                .map(|c| c.service_name())
                .collect::<HashSet<&String>>();

            running_services.retain(|config| !service_names.contains(config.service_name()));
        }

        for config in deployable_services {
            info!("started {} for {}.", config.service_name(), app_name);
            services.insert(app_name.clone(), config.clone());
        }
        Ok(services
            .get_vec(app_name)
            .unwrap()
            .iter()
            .map(|sc| Service {
                id: sc.service_name().clone(),
                config: ServiceConfig::clone(&sc),
                state: State {
                    status: ServiceStatus::Running,
                    started_at: DateTime::parse_from_rfc3339("2019-07-18T07:25:00.000000000Z")
                        .unwrap()
                        .with_timezone(&Utc),
                },
            })
            .collect::<Vec<_>>()
            .into())
    }

    async fn stop_services(&self, _status_id: &str, app_name: &AppName) -> Result<Services> {
        self.delay_if_configured().await;

        let mut services = self.services.lock().unwrap();

        match services.remove(&app_name) {
            Some(services) => Ok(Services::from(
                services
                    .into_iter()
                    .map(|sc| Service {
                        id: sc.service_name().clone(),
                        config: ServiceConfig::clone(&sc),
                        state: State {
                            status: ServiceStatus::Running,
                            started_at: DateTime::parse_from_rfc3339(
                                "2019-07-18T07:25:00.000000000Z",
                            )
                            .unwrap()
                            .with_timezone(&Utc),
                        },
                    })
                    .collect::<Vec<_>>(),
            )),
            None => Ok(Services::empty()),
        }
    }

    async fn get_logs<'a>(
        &'a self,
        app_name: &'a AppName,
        service_name: &'a str,
        _from: &'a Option<DateTime<FixedOffset>>,
        _limit: &'a Option<usize>,
        _follow: bool,
    ) -> BoxStream<'a, Result<(DateTime<FixedOffset>, String)>> {
        Box::pin(stream::iter(
            vec![
                (
                    DateTime::parse_from_rfc3339("2019-07-18T07:25:00.000000000Z").unwrap(),
                    format!("Log msg 1 of {} of app {}\n", service_name, app_name),
                ),
                (
                    DateTime::parse_from_rfc3339("2019-07-18T07:30:00.000000000Z").unwrap(),
                    format!("Log msg 2 of {} of app {}\n", service_name, app_name),
                ),
                (
                    DateTime::parse_from_rfc3339("2019-07-18T07:35:00.000000000Z").unwrap(),
                    format!("Log msg 3 of {} of app {}\n", service_name, app_name),
                ),
            ]
            .into_iter()
            .map(|s| Ok(s)),
        ))
    }

    async fn change_status(
        &self,
        _app_name: &AppName,
        _service_name: &str,
        _status: ServiceStatus,
    ) -> Result<Option<Service>> {
        Ok(None)
    }

    async fn base_traefik_ingress_route(&self) -> Result<Option<TraefikIngressRoute>> {
        Ok(self.base_ingress_route.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn http_forwarder(&self) -> Result<Box<dyn super::HttpForwarder + Send>> {
        unimplemented!("Currently not supported by the dummy infra")
    }
}
