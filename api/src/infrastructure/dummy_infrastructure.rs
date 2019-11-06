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
use crate::models::service::{Service, ServiceStatus};
use crate::models::ServiceConfig;
use chrono::{DateTime, FixedOffset, Utc};
use multimap::MultiMap;
use std::collections::HashSet;
use std::sync::Mutex;

#[cfg(test)]
pub struct DummyInfrastructure {
    services: Mutex<MultiMap<String, ServiceConfig>>,
}

#[cfg(test)]
impl DummyInfrastructure {
    pub fn new() -> DummyInfrastructure {
        DummyInfrastructure {
            services: Mutex::new(MultiMap::new()),
        }
    }
}

#[cfg(test)]
impl Infrastructure for DummyInfrastructure {
    fn get_services(&self) -> Result<MultiMap<String, Service>, failure::Error> {
        let mut s = MultiMap::new();

        let services = self.services.lock().unwrap();
        for (app, configs) in services.iter_all() {
            for config in configs {
                s.insert(
                    app.clone(),
                    Service::new(
                        format!("{}-{}", app.clone(), config.service_name()),
                        app.clone(),
                        config.service_name().clone(),
                        config.container_type().clone(),
                        crate::models::service::ServiceStatus::Running,
                        DateTime::parse_from_rfc3339("2019-07-18T07:30:00.000000000Z")
                            .unwrap()
                            .with_timezone(&Utc),
                    ),
                );
            }
        }

        Ok(s)
    }

    fn deploy_services(
        &self,
        app_name: &String,
        configs: &Vec<ServiceConfig>,
        _container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, failure::Error> {
        let mut services = self.services.lock().unwrap();

        if let Some(running_services) = services.get_vec_mut(app_name) {
            let service_names = configs
                .iter()
                .map(|c| c.service_name())
                .collect::<HashSet<&String>>();

            running_services.retain(|config| !service_names.contains(config.service_name()));
        }

        for config in configs {
            info!("started {} for {}.", config.service_name(), app_name);
            services.insert(app_name.clone(), config.clone());
        }
        Ok(vec![])
    }

    fn stop_services(&self, app_name: &String) -> Result<Vec<Service>, failure::Error> {
        let mut services = self.services.lock().unwrap();
        services.remove(app_name);
        Ok(vec![])
    }

    fn get_configs_of_app(&self, app_name: &String) -> Result<Vec<ServiceConfig>, failure::Error> {
        let services = self.services.lock().unwrap();
        match services.get_vec(app_name) {
            None => Ok(vec![]),
            Some(configs) => Ok(configs.clone()),
        }
    }

    fn get_logs(
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

    fn change_status(
        &self,
        _app_name: &String,
        _service_name: &String,
        _status: ServiceStatus,
    ) -> Result<Option<Service>, failure::Error> {
        Ok(None)
    }
}
