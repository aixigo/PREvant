/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2020 aixigo AG
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
use crate::config::{Config, ConfigError};
use crate::models::{AppName, ContainerType, Image, ServiceConfig};
use crate::services::service_templating::{
    apply_templating_for_application_companion, apply_templating_for_service_companion,
};
use handlebars::TemplateRenderError;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;

pub(super) struct DeploymentUnit {
    app_name: AppName,
    configs: Vec<ServiceConfig>,
    service_companions: Vec<(usize, ServiceConfig)>,
    app_companions: Vec<ServiceConfig>,
}

impl DeploymentUnit {
    pub fn new(app_name: AppName, configs: Vec<ServiceConfig>) -> Self {
        DeploymentUnit {
            app_name,
            configs,
            service_companions: Vec::new(),
            app_companions: Vec::new(),
        }
    }

    /// Extends the configuration with configuration options, such as:
    ///
    /// - secrets
    /// - application and service companions
    pub fn extend_with_config(&mut self, config: &Config) -> Result<(), ConfigError> {
        for service_config in self.configs.iter_mut() {
            config.add_secrets_to(service_config, &self.app_name);
        }

        let service_companions = self.merge_service_configs_with_companions(
            config.service_companion_configs(&self.app_name)?,
        );
        for service_companion in service_companions {
            for (index, _service_config) in self.configs.iter().enumerate() {
                self.service_companions
                    .push((index, service_companion.clone()));
            }
        }

        let app_companions = self.merge_service_configs_with_companions(
            config.application_companion_configs(&self.app_name)?,
        );
        self.app_companions.extend(app_companions);

        Ok(())
    }

    /// Merges the `ServiceConfig`s with the corresponding companion config (based on the service name).
    ///
    /// Returns the remaining companions that do not match to any existing `ServiceConfig`.
    fn merge_service_configs_with_companions(
        &mut self,
        companions: Vec<ServiceConfig>,
    ) -> Vec<ServiceConfig> {
        for service in self.configs.iter_mut() {
            let matching_companion = companions
                .iter()
                .find(|companion| companion.service_name() == service.service_name());

            if let Some(matching_companion) = matching_companion {
                service.merge_with(matching_companion);
            }
        }

        companions
            .into_iter()
            .filter(|companion| {
                self.configs
                    .iter()
                    .find(|c| companion.service_name() == c.service_name())
                    .is_none()
            })
            .collect()
    }

    pub fn images(&self) -> HashSet<Image> {
        let mut images = HashSet::new();

        images.extend(self.configs.iter().map(|config| config.image().clone()));
        images.extend(
            self.service_companions
                .iter()
                .map(|(_, config)| config.image().clone()),
        );
        images.extend(
            self.app_companions
                .iter()
                .map(|config| config.image().clone()),
        );

        images
    }

    pub fn assign_port_mappings(&mut self, port_mappings: &HashMap<Image, u16>) {
        Self::assign_port_mappings_impl(self.configs.iter_mut(), port_mappings);
        Self::assign_port_mappings_impl(
            self.service_companions.iter_mut().map(|(_, config)| config),
            port_mappings,
        );
        Self::assign_port_mappings_impl(self.app_companions.iter_mut(), port_mappings);
    }

    fn assign_port_mappings_impl<'a, Iter>(configs: Iter, port_mappings: &HashMap<Image, u16>)
    where
        Iter: Iterator<Item = &'a mut ServiceConfig>,
    {
        for config in configs {
            if let Some(port) = port_mappings.get(config.image()) {
                config.set_port(port.clone());
            }
        }
    }
}

impl TryInto<Vec<ServiceConfig>> for DeploymentUnit {
    type Error = TemplateRenderError;

    fn try_into(self) -> Result<Vec<ServiceConfig>, Self::Error> {
        let mut configs = Vec::new();

        for (index, companion_config) in self.service_companions.into_iter() {
            let companion_config = apply_templating_for_service_companion(
                &companion_config,
                &self.app_name,
                &self.configs[index],
            )?;
            configs.push(companion_config);
        }

        configs.extend(self.configs);

        for companion_config in self.app_companions.into_iter() {
            let companion_config = apply_templating_for_application_companion(
                &companion_config,
                &self.app_name,
                &configs,
            )?;
            configs.push(companion_config);
        }

        configs.sort_unstable_by(|a, b| {
            let index1 = container_type_index(a.container_type());
            let index2 = container_type_index(b.container_type());
            index1.cmp(&index2)
        });

        Ok(configs)
    }
}

fn container_type_index(container_type: &ContainerType) -> i32 {
    match container_type {
        ContainerType::ApplicationCompanion => 0,
        ContainerType::ServiceCompanion => 1,
        ContainerType::Instance | ContainerType::Replica => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EnvironmentVariable;
    use crate::{config_from_str, sc};
    use secstr::SecUtf8;
    use std::str::FromStr;

    #[test]
    fn should_return_unique_images() {
        let unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![sc!("http1", "nginx:1.13"), sc!("http2", "nginx:1.13")],
        );

        let images = unit.images();

        assert_eq!(images.len(), 1);
    }

    #[test]
    fn should_apply_port_mappings() {
        let config = config_from_str!(
            r#"
            [companions.http2]
            serviceName = 'http2'
            type = 'application'
            image = 'nginx:1.13'

            [companions.http3]
            serviceName = 'http3'
            type = 'service'
            image = 'nginx:1.13'
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![sc!("http1", "nginx:1.13")],
        );
        unit.extend_with_config(&config).unwrap();

        let mut port_mappings = HashMap::new();
        port_mappings.insert(Image::from_str("nginx:1.13").unwrap(), 4711);

        unit.assign_port_mappings(&port_mappings);

        let configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(configs[0].port(), 4711);
        assert_eq!(configs[1].port(), 4711);
        assert_eq!(configs[2].port(), 4711);
    }

    #[test]
    fn should_merge_with_application_companion_if_services_contain_same_service_name() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ "VAR_1=abcd", "VAR_2=1234" ]
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![sc!(
                "openid",
                labels = (),
                env = ("VAR_1" => "efg"),
                volumes = ()
            )],
        );

        unit.extend_with_config(&config).unwrap();

        let openid_configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(openid_configs.len(), 1);
        let openid_env = openid_configs[0].env().unwrap();

        assert_eq!(
            openid_env.variable("VAR_1"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_1"),
                SecUtf8::from("efg")
            ))
        );
        assert_eq!(
            openid_env.variable("VAR_2"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_2"),
                SecUtf8::from("1234")
            ))
        );
    }

    #[test]
    fn should_merge_with_service_companion_if_services_contain_same_service_name() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'service'
            image = 'private.example.com/library/openid:latest'
            env = [ "VAR_1=abcd", "VAR_2=1234" ]

            [companions.openid.labels]
            'traefik.frontend.rule' = 'PathPrefix:/example.com/openid/;'
            'traefik.frontend.priority' = '20000'
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![sc!(
                "openid",
                labels = (),
                env = ("VAR_1" => "efg"),
                volumes = ()
            )],
        );

        unit.extend_with_config(&config).unwrap();

        let openid_configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(openid_configs.len(), 1);
        let openid_env = openid_configs[0].env().unwrap();

        assert_eq!(
            openid_env.variable("VAR_1"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_1"),
                SecUtf8::from("efg")
            ))
        );
        assert_eq!(
            openid_env.variable("VAR_2"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_2"),
                SecUtf8::from("1234")
            ))
        );
    }

    #[test]
    fn should_apply_templating_on_service_companions() {
        let config = config_from_str!(
            r#"
            [companions.db]
            serviceName = '{{service.name}}-db'
            type = 'service'
            image = 'postgres:11'
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![
                sc!("wordpress", "wordpress:alpine"),
                sc!("nextcloud", "nextcloud:alpine"),
            ],
        );
        unit.extend_with_config(&config).unwrap();

        let configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(configs.len(), 4);

        assert_eq!(
            configs
                .iter()
                .find(|config| config.service_name() == "wordpress-db")
                .map(|config| config.image().to_string()),
            Some("docker.io/library/postgres:11".to_string())
        );
        assert_eq!(
            configs
                .iter()
                .find(|config| config.service_name() == "nextcloud-db")
                .map(|config| config.image().to_string()),
            Some("docker.io/library/postgres:11".to_string())
        );
    }

    #[test]
    fn should_apply_templating_on_app_companions() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = '{{application.name}}-openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![sc!("wordpress", "wordpress:alpine")],
        );
        unit.extend_with_config(&config).unwrap();

        let configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(configs.len(), 2);

        assert_eq!(
            configs
                .iter()
                .find(|config| config.container_type() == &ContainerType::ApplicationCompanion)
                .map(|config| config.service_name().clone()),
            Some("master-openid".to_string())
        );
    }
}
