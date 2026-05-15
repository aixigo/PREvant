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
use crate::infrastructure::Infrastructure;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use domain::{
    AppName,
    app_blueprints::{DesiredServiceStatus, ServiceConfig},
    app_deployment::{
        ApplicationCompanion, BootstrapCompanionsWithRawElementsContext, BootstrappedCompanions,
        DeployableService, DeploymentUnit,
    },
    app_instance::{App, Service, ServiceStatus},
    templating::{TemplateData, TemplatedClone},
};
use futures::stream::{self, BoxStream};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct DummyInfrastructure {
    delay: Option<Duration>,
    deployment_units: Arc<Mutex<HashMap<AppName, DeploymentUnit>>>,
    created_at: Arc<Mutex<HashMap<AppName, DateTime<Utc>>>>,
    bootstrapping_configs: Vec<ServiceConfig>,
}

#[cfg(test)]
impl DummyInfrastructure {
    pub fn new() -> Self {
        Self {
            delay: None,
            deployment_units: Arc::new(Mutex::new(HashMap::new())),
            created_at: Arc::new(Mutex::new(HashMap::new())),
            bootstrapping_configs: Vec::new(),
        }
    }

    pub fn with_delay(delay: Duration) -> Self {
        Self {
            delay: Some(delay),
            deployment_units: Arc::new(Mutex::new(HashMap::new())),
            created_at: Arc::new(Mutex::new(HashMap::new())),
            bootstrapping_configs: Vec::new(),
        }
    }

    pub fn services(&self) -> Vec<DeployableService> {
        self.deployment_units
            .lock()
            .unwrap()
            .values()
            .flat_map(|unit| unit.services.iter().cloned())
            .collect::<Vec<_>>()
    }

    pub fn with_existing_app(
        self,
        app_name: AppName,
        services: Vec<ServiceConfig>,
        created_at: DateTime<Utc>,
    ) -> Self {
        {
            use domain::app_deployment::AppDeploymentBuilder;

            let deployment_unit = AppDeploymentBuilder::init(app_name, services, None)
                .finish()
                .unwrap();

            self.deploy_fake_impl(&deployment_unit);

            let mut created_ats = self.created_at.lock().unwrap();
            created_ats.insert(deployment_unit.app_name.clone(), created_at);
        }

        self
    }

    fn deploy_fake_impl(&self, deployment_unit: &DeploymentUnit) {
        let app_name = &deployment_unit.app_name;

        {
            let mut units = self.deployment_units.lock().unwrap();
            units.insert(app_name.clone(), deployment_unit.clone());
        }
    }

    pub fn with_bootstrapping(mut self, bootstrapping_configs: Vec<ServiceConfig>) -> Self {
        self.bootstrapping_configs.extend(bootstrapping_configs);
        self
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

        let units = self.deployment_units.lock().unwrap();
        for (app_name, deployment_unit) in units.iter() {
            let mut services = Vec::with_capacity(deployment_unit.services.len());
            for service in &deployment_unit.services {
                let service = Service {
                    id: service.blueprint_service.service_name.clone(),
                    blueprint_config: service.blueprint_service.clone(),
                    status: ServiceStatus::Running {
                        started_at: DateTime::parse_from_rfc3339("2019-07-18T07:30:00.000000000Z")
                            .unwrap()
                            .with_timezone(&Utc),
                    },
                    service_type: service.service_type,
                };

                services.push(service);
            }

            let created_at = self.created_at.lock().unwrap();
            let created_at = created_at.get(app_name).cloned();
            apps.insert(
                app_name.clone(),
                App::new(
                    services,
                    deployment_unit.owners.clone(),
                    deployment_unit.user_defined_parameters.clone(),
                    created_at,
                ),
            );
        }

        Ok(apps)
    }

    async fn fetch_app(&self, app_name: &AppName) -> Result<Option<App>> {
        Ok(self.fetch_apps().await?.remove(app_name))
    }

    async fn deploy_services(
        &self,
        deployment_unit: &DeploymentUnit,
        _container_config: &ContainerConfig,
    ) -> Result<App> {
        self.delay_if_configured().await;

        self.deploy_fake_impl(deployment_unit);

        Ok(self
            .fetch_apps()
            .await?
            .remove(&deployment_unit.app_name)
            .unwrap())
    }

    async fn stop_services(&self, app_name: &AppName) -> Result<Option<App>> {
        self.delay_if_configured().await;

        let mut units = self.deployment_units.lock().unwrap();
        let (services, owners, user_defined_parameters) = match units.remove(app_name) {
            Some(unit) => (
                unit.services
                    .into_iter()
                    .map(|sc| Service {
                        id: sc.blueprint_service.service_name.clone(),
                        blueprint_config: sc.blueprint_service.clone(),
                        status: ServiceStatus::Running {
                            started_at: DateTime::parse_from_rfc3339(
                                "2019-07-18T07:25:00.000000000Z",
                            )
                            .unwrap()
                            .with_timezone(&Utc),
                        },
                        service_type: sc.service_type,
                    })
                    .collect::<Vec<_>>(),
                unit.owners,
                unit.user_defined_parameters,
            ),
            None => (Vec::new(), HashSet::new(), None),
        };

        let mut created_at = self.created_at.lock().unwrap();

        Ok(Some(App::new(
            services,
            owners,
            user_defined_parameters,
            created_at.remove(app_name),
        )))
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
        _status: DesiredServiceStatus,
    ) -> Result<Option<Service>> {
        Ok(None)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn http_forwarder(&self) -> Result<Box<dyn super::HttpForwarder>> {
        unimplemented!("Currently not supported by the dummy infra")
    }

    async fn bootstrap_companions_with_raw_elements(
        &self,
        _context: BootstrapCompanionsWithRawElementsContext<'_>,
        template_data: &TemplateData,
    ) -> Result<BootstrappedCompanions> {
        Ok(BootstrappedCompanions {
            bootstrapped_companions: self
                .bootstrapping_configs
                .iter()
                .map(|config| {
                    Ok(ApplicationCompanion::bootstrapped(
                        config
                            .templated_clone(template_data)
                            .map_err(|e| anyhow::anyhow!("Cannot template config: {e:?}"))?,
                        Vec::new(),
                    ))
                })
                .collect::<Result<Vec<_>>>()?,
            ..Default::default()
        })
    }
}
