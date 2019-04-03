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

use crate::models::service::ContainerType;
use crate::models::service::Service;
use crate::models::service::ServiceConfig;
use crate::services::config_service::ContainerConfig;
use crate::services::infrastructure::Infrastructure;
use multimap::MultiMap;
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
        for (app, config) in services.iter() {
            s.insert(
                app.clone(),
                Service::new(
                    app.clone(),
                    config.service_name().clone(),
                    ContainerType::Instance,
                ),
            );
        }

        Ok(s)
    }

    fn start_services(
        &self,
        app_name: &String,
        configs: &Vec<ServiceConfig>,
        _container_config: &ContainerConfig,
    ) -> Result<Vec<Service>, failure::Error> {
        let mut services = self.services.lock().unwrap();
        for config in configs {
            services.insert(app_name.clone(), config.clone());
        }
        Ok(vec![])
    }

    fn stop_services(&self, app_name: &String) -> Result<Vec<Service>, failure::Error> {
        let mut services = self.services.lock().unwrap();
        services.remove(app_name);
        Ok(vec![])
    }

    fn get_configs_of_app(&self, _app_name: &String) -> Result<Vec<ServiceConfig>, failure::Error> {
        Ok(vec![])
    }
}
