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
use crate::models::user_defined_parameters::UserDefinedParameters;
use crate::models::{App, AppName, Owner, Service, ServiceConfig, ServiceStatus, State};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use futures::stream::{self, BoxStream};
use log::info;
use multimap::MultiMap;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
#[derive(Clone)]
pub struct DummyInfrastructure {
    delay: Option<Duration>,
    services: Arc<Mutex<MultiMap<AppName, DeployableService>>>,
    user_defined_parameters: Arc<Mutex<HashMap<AppName, UserDefinedParameters>>>,
    owners: Arc<Mutex<MultiMap<AppName, Owner>>>,
}

#[cfg(test)]
impl DummyInfrastructure {
    pub fn new() -> Self {
        Self {
            delay: None,
            services: Arc::new(Mutex::new(MultiMap::new())),
            user_defined_parameters: Arc::new(Mutex::new(HashMap::new())),
            owners: Arc::new(Mutex::new(MultiMap::new())),
        }
    }

    pub fn with_delay(delay: Duration) -> Self {
        Self {
            delay: Some(delay),
            services: Arc::new(Mutex::new(MultiMap::new())),
            user_defined_parameters: Arc::new(Mutex::new(HashMap::new())),
            owners: Arc::new(Mutex::new(MultiMap::new())),
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
    async fn fetch_apps(&self) -> Result<HashMap<AppName, App>> {
        let mut apps = HashMap::new();

        let services = self.services.lock().unwrap();
        for (app, configs) in services.iter_all() {
            let mut services = Vec::with_capacity(configs.len());
            for config in configs {
                let service = Service {
                    id: config.service_name().clone(),
                    config: ServiceConfig::clone(config),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: Some(
                            DateTime::parse_from_rfc3339("2019-07-18T07:30:00.000000000Z")
                                .unwrap()
                                .with_timezone(&Utc),
                        ),
                    },
                };

                services.push(service);
            }

            let app_name = AppName::from_str(app).unwrap();
            let user_defined_parameters = self.user_defined_parameters.lock().unwrap();
            let udp = user_defined_parameters.get(&app_name).cloned();

            let owners = self.owners.lock().unwrap();
            let owners = owners
                .get_vec(&app_name)
                .map(|owners| owners.iter().cloned().collect::<HashSet<Owner>>())
                .unwrap_or_default();

            apps.insert(app_name, App::new(services, owners, udp));
        }

        Ok(apps)
    }

    async fn fetch_app(&self, app_name: &AppName) -> Result<Option<App>> {
        Ok(self.fetch_apps().await?.remove(app_name))
    }

    async fn deploy_services(
        &self,
        _status_id: &str,
        deployment_unit: &DeploymentUnit,
        _container_config: &ContainerConfig,
    ) -> Result<App> {
        self.delay_if_configured().await;

        let app_name = deployment_unit.app_name();

        {
            let mut services = self.services.lock().unwrap();
            let mut user_defined_parameters = self.user_defined_parameters.lock().unwrap();
            let mut owners = self.owners.lock().unwrap();

            if let Some(p) = deployment_unit.user_defined_parameters() {
                user_defined_parameters.insert(app_name.clone(), p.clone());
            }

            let deployable_services = deployment_unit.services();
            if let Some(running_services) = services.get_vec_mut(app_name) {
                let service_names = deployable_services
                    .iter()
                    .map(|c| c.service_name())
                    .collect::<HashSet<&String>>();

                running_services.retain(|config| !service_names.contains(config.service_name()));
            }

            owners.insert_many(app_name.clone(), deployment_unit.owners().iter().cloned());

            for config in deployable_services {
                info!("started {} for {}.", config.service_name(), app_name);
                services.insert(app_name.clone(), config.clone());
            }
        }

        Ok(self.fetch_apps().await?.remove(app_name).unwrap())
    }

    async fn stop_services(&self, _status_id: &str, app_name: &AppName) -> Result<App> {
        self.delay_if_configured().await;

        let mut services = self.services.lock().unwrap();
        let services = match services.remove(app_name) {
            Some(services) => services
                .into_iter()
                .map(|sc| Service {
                    id: sc.service_name().clone(),
                    config: ServiceConfig::clone(&sc),
                    state: State {
                        status: ServiceStatus::Running,
                        started_at: Some(
                            DateTime::parse_from_rfc3339("2019-07-18T07:25:00.000000000Z")
                                .unwrap()
                                .with_timezone(&Utc),
                        ),
                    },
                })
                .collect::<Vec<_>>(),
            None => return Ok(App::empty()),
        };

        let mut owners = self.owners.lock().unwrap();
        let owners = owners
            .remove(app_name)
            .map(|owners| owners.into_iter().collect::<HashSet<_>>())
            .unwrap_or_default();

        let mut user_defined_parameters = self.user_defined_parameters.lock().unwrap();
        let user_defined_parameters = user_defined_parameters.remove(app_name);

        Ok(App::new(services, owners, user_defined_parameters))
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
                    format!("Log msg 1 of {service_name} of app {app_name}\n"),
                ),
                (
                    DateTime::parse_from_rfc3339("2019-07-18T07:30:00.000000000Z").unwrap(),
                    format!("Log msg 2 of {service_name} of app {app_name}\n"),
                ),
                (
                    DateTime::parse_from_rfc3339("2019-07-18T07:35:00.000000000Z").unwrap(),
                    format!("Log msg 3 of {service_name} of app {app_name}\n"),
                ),
            ]
            .into_iter()
            .map(Ok),
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

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn http_forwarder(&self) -> Result<Box<dyn super::HttpForwarder>> {
        unimplemented!("Currently not supported by the dummy infra")
    }
}
