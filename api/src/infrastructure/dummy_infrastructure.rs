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
use crate::models::service::{Service, ServiceStatus};
use crate::models::{ServiceBuilder, ServiceConfig};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use multimap::MultiMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use super::TraefikIngressRoute;

#[cfg(test)]
pub struct DummyInfrastructure {
    delay: Option<Duration>,
    services: Mutex<MultiMap<String, DeployableService>>,
    base_ingress_route: Option<TraefikIngressRoute>,
}

#[cfg(test)]
impl DummyInfrastructure {
    pub fn new() -> Self {
        Self {
            delay: None,
            services: Mutex::new(MultiMap::new()),
            base_ingress_route: None,
        }
    }

    pub fn with_delay(delay: Duration) -> Self {
        Self {
            delay: Some(delay),
            services: Mutex::new(MultiMap::new()),
            base_ingress_route: None,
        }
    }

    pub fn with_base_route(base_ingress_route: TraefikIngressRoute) -> Self {
        Self {
            delay: None,
            services: Mutex::new(MultiMap::new()),
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
    async fn get_services(&self) -> Result<MultiMap<String, Service>, failure::Error> {
        let mut s = MultiMap::new();

        let services = self.services.lock().unwrap();
        for (app, configs) in services.iter_all() {
            for config in configs {
                let service = ServiceBuilder::new()
                    .id(format!("{}", config.service_name()))
                    .app_name(app.clone())
                    .config(ServiceConfig::clone(config))
                    .service_status(ServiceStatus::Running)
                    .started_at(
                        DateTime::parse_from_rfc3339("2019-07-18T07:30:00.000000000Z")
                            .unwrap()
                            .with_timezone(&Utc),
                    )
                    .build()
                    .unwrap();

                s.insert(app.clone(), service);
            }
        }

        Ok(s)
    }

    async fn deploy_services(
        &self,
        _status_id: &String,
        deployment_unit: &DeploymentUnit,
        _container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, failure::Error> {
        self.delay_if_configured().await;

        let mut services = self.services.lock().unwrap();
        let app_name = deployment_unit.app_name().to_string();
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
        Ok(vec![])
    }

    async fn stop_services(
        &self,
        _status_id: &String,
        app_name: &String,
    ) -> Result<Vec<Service>, failure::Error> {
        self.delay_if_configured().await;

        let mut services = self.services.lock().unwrap();
        match services.remove(app_name) {
            Some(services) => Ok(services
                .into_iter()
                .map(|sc| {
                    ServiceBuilder::new()
                        .app_name(app_name.clone())
                        .id(sc.service_name().clone())
                        .config(ServiceConfig::clone(&sc))
                        .started_at(
                            DateTime::parse_from_rfc3339("2019-07-18T07:25:00.000000000Z")
                                .unwrap()
                                .with_timezone(&Utc),
                        )
                        .build()
                        .unwrap()
                })
                .collect()),
            None => Ok(vec![]),
        }
    }

    async fn get_logs(
        &self,
        app_name: &String,
        service_name: &String,
        _from: &Option<DateTime<FixedOffset>>,
        _limit: usize,
    ) -> Result<Option<Vec<(DateTime<FixedOffset>, String)>>, failure::Error> {
        Ok(Some(vec![
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
        ]))
    }

    async fn change_status(
        &self,
        _app_name: &String,
        _service_name: &String,
        _status: ServiceStatus,
    ) -> Result<Option<Service>, failure::Error> {
        Ok(None)
    }

    async fn base_traefik_ingress_route(
        &self,
    ) -> Result<Option<TraefikIngressRoute>, failure::Error> {
        Ok(self.base_ingress_route.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
