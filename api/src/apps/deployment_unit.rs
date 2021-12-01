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
use crate::config::Config;
use crate::models::{AppName, ContainerType, Image, ServiceConfig};
use handlebars::TemplateRenderError;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;

pub(super) struct DeploymentUnit {
    app_name: AppName,
    configs: Vec<ServiceConfig>,
    service_companions: Vec<ServiceConfig>,
    app_companions: Vec<ServiceConfig>,
    templating_only_service_configs: Vec<ServiceConfig>,
}

impl DeploymentUnit {
    pub fn new(app_name: AppName, configs: Vec<ServiceConfig>) -> Self {
        DeploymentUnit {
            app_name,
            configs,
            service_companions: Vec::new(),
            app_companions: Vec::new(),
            templating_only_service_configs: Vec::new(),
        }
    }

    /// Extends the `DeploymentUnit` with configuration options, such as:
    ///
    /// - secrets
    /// - application and service companions
    pub fn extend_with_config(&mut self, config: &Config) {
        for service_config in self.configs.iter_mut() {
            config.add_secrets_to(service_config, &self.app_name);
        }

        let service_companions = config.service_companion_configs(&self.app_name);
        self.service_companions.extend(service_companions);

        let app_companions = config.application_companion_configs(&self.app_name);
        self.app_companions.extend(app_companions);
    }

    /// Extends the `DeploymentUnit` with service configuration that are only required for templating
    pub fn extend_with_templating_only_service_configs<ServiceConfigIter>(
        &mut self,
        configs: ServiceConfigIter,
    ) where
        ServiceConfigIter: IntoIterator<Item = ServiceConfig>,
    {
        self.templating_only_service_configs.extend(configs);
    }

    pub fn images(&self) -> HashSet<Image> {
        let mut images = HashSet::new();

        images.extend(self.configs.iter().map(|config| config.image().clone()));
        images.extend(
            self.service_companions
                .iter()
                .map(|config| config.image().clone()),
        );
        images.extend(
            self.app_companions
                .iter()
                .map(|config| config.image().clone()),
        );
        images.extend(
            self.templating_only_service_configs
                .iter()
                .map(|config| config.image().clone()),
        );

        images
    }

    pub fn assign_port_mappings(&mut self, port_mappings: &HashMap<Image, u16>) {
        Self::assign_port_mappings_impl(self.configs.iter_mut(), port_mappings);
        Self::assign_port_mappings_impl(self.service_companions.iter_mut(), port_mappings);
        Self::assign_port_mappings_impl(self.app_companions.iter_mut(), port_mappings);
        Self::assign_port_mappings_impl(
            self.templating_only_service_configs.iter_mut(),
            port_mappings,
        );
    }

    fn assign_port_mappings_impl<'a, Iter>(configs: Iter, port_mappings: &HashMap<Image, u16>)
    where
        Iter: Iterator<Item = &'a mut ServiceConfig>,
    {
        for config in configs {
            if let Some(port) = port_mappings.get(config.image()) {
                config.set_port(*port);
            }
        }
    }
}

impl TryInto<Vec<ServiceConfig>> for DeploymentUnit {
    type Error = TemplateRenderError;

    fn try_into(self) -> Result<Vec<ServiceConfig>, Self::Error> {
        let mut services = HashMap::new();

        for config in self.configs.into_iter() {
            services.insert(
                config.service_name().clone(),
                config.apply_templating(&self.app_name)?,
            );
        }

        // If the user wants to deploy a service that has the same name as a companion,
        // it must be avoided that services will be deployed twice. Furthermore,
        // deploying service companions should be avoided for services that override a
        // service companion.

        struct ServiceCompanion {
            templated_companion: ServiceConfig,
            for_service_name: String,
        }

        let mut service_companions = Vec::new();
        for service in services.values() {
            for service_companion in self.service_companions.iter() {
                let templated_companion = service_companion
                    .apply_templating_for_service_companion(&self.app_name, service)?;

                service_companions.push(ServiceCompanion {
                    templated_companion,
                    for_service_name: service.service_name().clone(),
                });
            }
        }

        let (service_companions_of_request, service_companions_of_config): (Vec<_>, Vec<_>) =
            service_companions
                .into_iter()
                .partition(|service_companion| {
                    services
                        .get_mut(service_companion.templated_companion.service_name())
                        .is_some()
                });

        for companion in service_companions_of_request.iter() {
            services
                .get_mut(companion.templated_companion.service_name())
                .unwrap()
                .merge_with(&companion.templated_companion);
        }

        // Exclude service_companions that are included in the request
        services.extend(
            service_companions_of_config
                .into_iter()
                .filter(|service_companion| {
                    service_companions_of_request
                        .iter()
                        .find(|scor| {
                            &service_companion.for_service_name
                                == scor.templated_companion.service_name()
                        })
                        .is_none()
                })
                .map(|service_companion| {
                    (
                        service_companion.templated_companion.service_name().clone(),
                        service_companion.templated_companion,
                    )
                }),
        );

        let mut templating_only_service_configs = self.templating_only_service_configs;
        templating_only_service_configs.extend(services.values().cloned());
        for companion_config in self.app_companions.into_iter() {
            let companion_config = companion_config.apply_templating_for_application_companion(
                &self.app_name,
                &templating_only_service_configs,
            )?;

            // If a custom application companion was deployed, its config needs to be merged
            // with the companion config
            let existing_config = services.get_mut(companion_config.service_name());

            if let Some(existing_config) = existing_config {
                existing_config.merge_with(&companion_config);
            } else {
                services.insert(companion_config.service_name().clone(), companion_config);
            }
        }

        let mut configs: Vec<_> = services.into_iter().map(|(_, config)| config).collect();
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
    use crate::models::{Environment, EnvironmentVariable};
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
        unit.extend_with_config(&config);

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

        unit.extend_with_config(&config);

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

        unit.extend_with_config(&config);

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
        unit.extend_with_config(&config);

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
    fn should_apply_templating_on_service_environment_variables() {
        let mut config = sc!("wordpress-db", "mariadb:10.3.17");
        config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_templating(
                String::from("MYSQL_DATABASE"),
                SecUtf8::from("{{service.name}}"),
            ),
        ])));

        let unit = DeploymentUnit::new(AppName::from_str("master").unwrap(), vec![config]);

        let configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(configs.len(), 1);
        let env = configs[0].env().unwrap();

        assert_eq!(
            env.variable("MYSQL_DATABASE"),
            Some(&EnvironmentVariable::new(
                String::from("MYSQL_DATABASE"),
                SecUtf8::from("wordpress-db")
            ))
        );
    }

    #[test]
    fn should_not_apply_templating_on_service_environment_variables() {
        let mut config = sc!("wordpress-db", "mariadb:10.3.17");
        config.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("MYSQL_DATABASE"),
            SecUtf8::from("{{application.name}}"),
        )])));

        let unit = DeploymentUnit::new(AppName::from_str("master").unwrap(), vec![config]);

        let configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(configs.len(), 1);
        let env = configs[0].env().unwrap();

        assert_eq!(
            env.variable("MYSQL_DATABASE"),
            Some(&EnvironmentVariable::new(
                String::from("MYSQL_DATABASE"),
                SecUtf8::from("{{application.name}}")
            ))
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
            env = [ """SERVICES={{~#each services~}}{{name}},{{~/each~}}""" ]
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![sc!("wordpress", "wordpress:alpine")],
        );
        unit.extend_with_config(&config);

        let configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(configs.len(), 2);

        assert_eq!(
            configs
                .iter()
                .find(|config| config.container_type() == &ContainerType::ApplicationCompanion)
                .map(|config| config.service_name().clone()),
            Some("master-openid".to_string())
        );

        assert_eq!(
            configs
                .iter()
                .find(|config| config.container_type() == &ContainerType::ApplicationCompanion)
                .map(|config| config
                    .env()
                    .unwrap()
                    .get(0)
                    .unwrap()
                    .value()
                    .unsecure()
                    .to_string()),
            Some("wordpress,".to_string())
        );
    }

    #[test]
    fn should_apply_templating_on_app_companions_with_templating_only_configs() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = '{{application.name}}-openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ """SERVICES={{~#each services~}}{{name}},{{~/each~}}""" ]
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![sc!("wordpress", "wordpress:alpine")],
        );
        unit.extend_with_config(&config);
        unit.extend_with_templating_only_service_configs(vec![sc!("postgres", "postgres:alpine")]);

        let configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(configs.len(), 2);

        assert_eq!(
            configs
                .iter()
                .find(|config| config.container_type() == &ContainerType::ApplicationCompanion)
                .map(|config| config.service_name().clone()),
            Some("master-openid".to_string())
        );

        assert_eq!(
            configs
                .iter()
                .find(|config| config.container_type() == &ContainerType::ApplicationCompanion)
                .map(|config| config
                    .env()
                    .unwrap()
                    .get(0)
                    .unwrap()
                    .value()
                    .unsecure()
                    .to_string()),
            Some("postgres,wordpress,".to_string())
        );
    }

    #[test]
    fn should_apply_templating_on_merged_application_companions() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ "VAR_1={{application.name}}" ]
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![sc!("openid", "private.example.com/library/openid:backup")],
        );

        unit.extend_with_config(&config);

        let openid_configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(openid_configs.len(), 1);
        let openid_env = openid_configs[0].env().unwrap();

        assert_eq!(
            openid_env.variable("VAR_1"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_1"),
                SecUtf8::from("master")
            ))
        );
        assert_eq!(
            openid_configs[0].image().to_string(),
            "private.example.com/library/openid:backup".to_string()
        );
    }

    #[test]
    fn should_apply_templating_on_merged_service_companions() {
        let config = config_from_str!(
            r#"
            [companions.db]
            serviceName = '{{service.name}}-db'
            type = 'service'
            image = 'postgres:11'
            env = [ "VAR_1={{application.name}}" ]

            [companions.kafka]
            serviceName = '{{service.name}}-kafka'
            type = 'service'
            image = 'kafka'
        "#
        );

        let mut unit = DeploymentUnit::new(
            AppName::from_str("master").unwrap(),
            vec![
                sc!("wordpress", "wordpress:alpine"),
                sc!("wordpress-db", "postgres:11-alpine"),
            ],
        );

        unit.extend_with_config(&config);

        let configs: Vec<_> = unit.try_into().unwrap();
        assert_eq!(configs.len(), 3);

        assert_eq!(
            configs
                .iter()
                .find(|config| config.service_name() == "wordpress-db")
                .map(|config| config.image().to_string()),
            Some("docker.io/library/postgres:11-alpine".to_string())
        );
        assert_eq!(
            configs
                .iter()
                .find(|config| config.service_name() == "wordpress-db")
                .and_then(|config| config.env().unwrap().variable("VAR_1")),
            Some(&EnvironmentVariable::new(
                String::from("VAR_1"),
                SecUtf8::from("master")
            ))
        );
        assert_eq!(
            configs
                .iter()
                .find(|config| config.service_name() == "wordpress-kafka")
                .map(|config| config.image().to_string()),
            Some("docker.io/library/kafka:latest".to_string())
        );
    }
}
