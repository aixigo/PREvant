use url::Url;

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
use crate::apps::AppsServiceError;
use crate::config::{Config, StorageStrategy};
use crate::deployment::hooks::Hooks;
use crate::infrastructure::{TraefikIngressRoute, TraefikMiddleware, TraefikRouterRule};
use crate::models::{AppName, ContainerType, Image, ServiceConfig};
use crate::registry::ImageInfo;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

pub struct Initialized {
    app_name: AppName,
    configs: Vec<ServiceConfig>,
}

pub struct WithCompanions {
    app_name: AppName,
    configs: Vec<ServiceConfig>,
    service_companions: Vec<(
        ServiceConfig,
        crate::config::DeploymentStrategy,
        crate::config::StorageStrategy,
    )>,
    app_companions: Vec<(
        ServiceConfig,
        crate::config::DeploymentStrategy,
        crate::config::StorageStrategy,
    )>,
}

pub struct WithTemplatedConfigs {
    app_name: AppName,
    configs: Vec<ServiceConfig>,
    service_companions: Vec<(
        ServiceConfig,
        crate::config::DeploymentStrategy,
        crate::config::StorageStrategy,
    )>,
    app_companions: Vec<(
        ServiceConfig,
        crate::config::DeploymentStrategy,
        crate::config::StorageStrategy,
    )>,
    templating_only_service_configs: Vec<ServiceConfig>,
}

pub struct WithResolvedImages {
    app_name: AppName,
    configs: Vec<ServiceConfig>,
    service_companions: Vec<(
        ServiceConfig,
        crate::config::DeploymentStrategy,
        crate::config::StorageStrategy,
    )>,
    app_companions: Vec<(
        ServiceConfig,
        crate::config::DeploymentStrategy,
        crate::config::StorageStrategy,
    )>,
    templating_only_service_configs: Vec<ServiceConfig>,
    image_infos: HashMap<Image, ImageInfo>,
}

pub struct WithAppliedTemplating {
    app_name: AppName,
    services: Vec<DeployableService>,
}

pub struct WithAppliedHooks {
    app_name: AppName,
    services: Vec<DeployableService>,
}

pub struct WithAppliedIngressRoute {
    app_name: AppName,
    services: Vec<DeployableService>,
    route: TraefikIngressRoute,
}

pub struct DeploymentUnitBuilder<Stage> {
    stage: Stage,
}

pub struct DeploymentUnit {
    app_name: AppName,
    services: Vec<DeployableService>,
    route: TraefikIngressRoute,
}

#[derive(Clone, Debug)]
pub enum DeploymentStrategy {
    RedeployAlways,
    RedeployOnImageUpdate(String),
    RedeployNever,
}

#[derive(Debug, Clone)]
pub struct DeployableService {
    raw_service_config: ServiceConfig,
    strategy: DeploymentStrategy,
    ingress_route: TraefikIngressRoute,
    declared_volumes: Vec<String>,
}

impl DeployableService {
    #[cfg(test)]
    pub fn new(
        raw_service_config: ServiceConfig,
        strategy: DeploymentStrategy,
        ingress_route: TraefikIngressRoute,
        declared_volumes: Vec<String>,
    ) -> Self {
        Self {
            raw_service_config,
            strategy,
            ingress_route,
            declared_volumes,
        }
    }

    pub fn strategy(&self) -> &DeploymentStrategy {
        &self.strategy
    }

    pub fn ingress_route(&self) -> &TraefikIngressRoute {
        &self.ingress_route
    }

    pub fn declared_volumes(&self) -> &Vec<String> {
        &self.declared_volumes
    }
}

impl std::ops::Deref for DeployableService {
    type Target = ServiceConfig;

    fn deref(&self) -> &Self::Target {
        &self.raw_service_config
    }
}

impl std::ops::DerefMut for DeployableService {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.raw_service_config
    }
}

impl DeploymentUnit {
    pub fn services(&self) -> &[DeployableService] {
        &self.services
    }

    pub fn app_name(&self) -> &AppName {
        &self.app_name
    }

    pub fn app_base_route(&self) -> &TraefikIngressRoute {
        &self.route
    }
}

impl DeploymentUnitBuilder<Initialized> {
    pub fn init(
        app_name: AppName,
        configs: Vec<ServiceConfig>,
    ) -> DeploymentUnitBuilder<Initialized> {
        DeploymentUnitBuilder {
            stage: Initialized { app_name, configs },
        }
    }
}

impl DeploymentUnitBuilder<Initialized> {
    pub fn extend_with_config(mut self, config: &Config) -> DeploymentUnitBuilder<WithCompanions> {
        for service_config in self.stage.configs.iter_mut() {
            config.add_secrets_to(service_config, &self.stage.app_name);
        }

        let service_companions = config.service_companion_configs(&self.stage.app_name);
        let app_companions = config.application_companion_configs(&self.stage.app_name);

        DeploymentUnitBuilder {
            stage: WithCompanions {
                app_name: self.stage.app_name,
                configs: self.stage.configs,
                service_companions,
                app_companions,
            },
        }
    }
}

impl DeploymentUnitBuilder<WithCompanions> {
    pub fn extend_with_templating_only_service_configs(
        self,
        templating_only_service_configs: Vec<ServiceConfig>,
    ) -> DeploymentUnitBuilder<WithTemplatedConfigs> {
        DeploymentUnitBuilder {
            stage: WithTemplatedConfigs {
                app_name: self.stage.app_name,
                configs: self.stage.configs,
                service_companions: self.stage.service_companions,
                app_companions: self.stage.app_companions,
                templating_only_service_configs,
            },
        }
    }
}

impl DeploymentUnitBuilder<WithTemplatedConfigs> {
    pub fn images(&self) -> HashSet<Image> {
        let mut images = HashSet::new();

        images.extend(
            self.stage
                .configs
                .iter()
                .map(|config| config.image().clone()),
        );
        images.extend(
            self.stage
                .service_companions
                .iter()
                .map(|(config, _, _)| config.image().clone()),
        );
        images.extend(
            self.stage
                .app_companions
                .iter()
                .map(|(config, _, _)| config.image().clone()),
        );
        images.extend(
            self.stage
                .templating_only_service_configs
                .iter()
                .map(|config| config.image().clone()),
        );

        images
    }

    pub fn extend_with_image_infos(
        mut self,
        image_infos: HashMap<Image, ImageInfo>,
    ) -> DeploymentUnitBuilder<WithResolvedImages> {
        Self::assign_port_mappings_impl(self.stage.configs.iter_mut(), &image_infos);
        Self::assign_port_mappings_impl(
            self.stage
                .service_companions
                .iter_mut()
                .map(|(companion, _, _)| companion),
            &image_infos,
        );
        Self::assign_port_mappings_impl(
            self.stage
                .app_companions
                .iter_mut()
                .map(|(companion, _, _)| companion),
            &image_infos,
        );
        Self::assign_port_mappings_impl(
            self.stage.templating_only_service_configs.iter_mut(),
            &image_infos,
        );

        DeploymentUnitBuilder {
            stage: WithResolvedImages {
                app_name: self.stage.app_name,
                configs: self.stage.configs,
                service_companions: self.stage.service_companions,
                app_companions: self.stage.app_companions,
                templating_only_service_configs: self.stage.templating_only_service_configs,
                image_infos,
            },
        }
    }

    fn assign_port_mappings_impl<'a, Iter>(configs: Iter, image_infos: &HashMap<Image, ImageInfo>)
    where
        Iter: Iterator<Item = &'a mut ServiceConfig>,
    {
        for config in configs {
            if let Some(info) = image_infos.get(config.image()) {
                if let Some(port) = info.exposed_port() {
                    config.set_port(port);
                }
            }
        }
    }
}

impl DeploymentUnitBuilder<WithResolvedImages> {
    pub fn apply_templating(
        self,
        base_url: &Option<Url>,
    ) -> Result<DeploymentUnitBuilder<WithAppliedTemplating>, AppsServiceError> {
        let mut services = HashMap::new();

        for config in self.stage.configs.iter() {
            let templated_config = config.apply_templating(&self.stage.app_name, base_url)?;

            services.insert(
                config.service_name().clone(),
                DeployableService {
                    raw_service_config: templated_config,
                    strategy: DeploymentStrategy::RedeployAlways,
                    ingress_route: TraefikIngressRoute::with_defaults(
                        &self.stage.app_name,
                        config.service_name(),
                    ),
                    declared_volumes: Vec::new(),
                },
            );
        }

        // If the user wants to deploy a service that has the same name as a companion,
        // it must be avoided that services will be deployed twice. Furthermore,
        // deploying service companions should be avoided for services that override a
        // service companion.

        struct ServiceCompanion<'a> {
            templated_companion: ServiceConfig,
            strategy: &'a crate::config::DeploymentStrategy,
            storage_strategy: &'a crate::config::StorageStrategy,
            for_service_name: String,
        }

        let mut service_companions = Vec::new();
        for service in services.values() {
            for (service_companion, strategy, storage_strategy) in
                self.stage.service_companions.iter()
            {
                let templated_companion = service_companion
                    .apply_templating_for_service_companion(
                        &self.stage.app_name,
                        base_url,
                        service,
                    )?;

                service_companions.push(ServiceCompanion {
                    templated_companion,
                    strategy,
                    for_service_name: service.service_name().clone(),
                    storage_strategy,
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

        let image_infos = &self.stage.image_infos;

        // Exclude service_companions that are included in the request
        services.extend(
            service_companions_of_config
                .into_iter()
                .filter(|service_companion| {
                    !service_companions_of_request.iter().any(|scor| {
                        &service_companion.for_service_name
                            == scor.templated_companion.service_name()
                    })
                })
                .map(|service_companion| {
                    Ok((
                        service_companion.templated_companion.service_name().clone(),
                        self.deployable_service(
                            service_companion.templated_companion,
                            service_companion.strategy,
                            service_companion.storage_strategy,
                            image_infos,
                        )?,
                    ))
                })
                .collect::<Result<Vec<_>, AppsServiceError>>()?,
        );

        let mut templating_only_service_configs =
            self.stage.templating_only_service_configs.clone();
        templating_only_service_configs.extend(
            services
                .values()
                .map(|strategy| ServiceConfig::clone(strategy)),
        );
        for (companion_config, strategy, storage_strategy) in self.stage.app_companions.iter() {
            let companion_config = companion_config.apply_templating_for_application_companion(
                &self.stage.app_name,
                base_url,
                &templating_only_service_configs,
            )?;

            // If a custom application companion was deployed, its config needs to be merged
            // with the companion config
            let existing_config = services.get_mut(companion_config.service_name());

            if let Some(existing_strategy) = existing_config {
                existing_strategy.merge_with(&companion_config);
            } else {
                services.insert(
                    companion_config.service_name().clone(),
                    self.deployable_service(
                        companion_config,
                        strategy,
                        storage_strategy,
                        image_infos,
                    )?,
                );
            }
        }

        let mut strategies = services.into_values().collect::<Vec<_>>();

        strategies.sort_unstable_by(|a, b| {
            let index1 = Self::container_type_index(a.container_type());
            let index2 = Self::container_type_index(b.container_type());
            index1.cmp(&index2)
        });

        Ok(DeploymentUnitBuilder {
            stage: WithAppliedTemplating {
                app_name: self.stage.app_name,
                services: strategies,
            },
        })
    }

    fn deployable_service(
        &self,
        raw_service_config: ServiceConfig,
        strategy: &crate::config::DeploymentStrategy,
        storage_strategy: &StorageStrategy,
        image_infos: &HashMap<Image, ImageInfo>,
    ) -> Result<DeployableService, AppsServiceError> {
        let ingress_route =
            match raw_service_config.routing() {
                None => TraefikIngressRoute::with_defaults(
                    &self.stage.app_name,
                    raw_service_config.service_name(),
                ),
                Some(routing) => match &routing.rule {
                    Some(rule) => TraefikIngressRoute::with_rule_and_middlewares(
                        TraefikRouterRule::from_str(rule).map_err(|err| {
                            AppsServiceError::FailedToParseTraefikRule {
                                raw_rule: rule.clone(),
                                err,
                            }
                        })?,
                        routing
                            .additional_middlewares
                            .iter()
                            .enumerate()
                            .map(|(i, (name, spec))| TraefikMiddleware {
                                name: format!("custom-middleware-{i}"),
                                spec: serde_value::to_value(serde_json::json!({
                                    name: spec.clone()
                                }))
                                .unwrap(),
                            })
                            .collect::<Vec<_>>(),
                    ),
                    None => TraefikIngressRoute::with_defaults_and_additional_middleware(
                        &self.stage.app_name,
                        raw_service_config.service_name(),
                        routing.additional_middlewares.iter().enumerate().map(
                            |(i, (name, spec))| TraefikMiddleware {
                                name: format!("custom-middleware-{i}"),
                                spec: serde_value::to_value(serde_json::json!({
                                    name: spec.clone()
                                }))
                                .unwrap(),
                            },
                        ),
                    ),
                },
            };

        let volume_paths = match image_infos.get(raw_service_config.image()) {
            None => Vec::new(),
            Some(info) => info.declared_volumes(),
        };

        let declared_volumes = match storage_strategy {
            StorageStrategy::NoMountVolumes => Vec::new(),
            StorageStrategy::MountDeclaredImageVolumes => volume_paths
                .into_iter()
                .map(|path| path.to_owned())
                .collect(),
        };

        Ok(match strategy {
            crate::config::DeploymentStrategy::RedeployAlways => DeployableService {
                raw_service_config,
                ingress_route,
                strategy: DeploymentStrategy::RedeployAlways,
                declared_volumes,
            },
            crate::config::DeploymentStrategy::RedeployOnImageUpdate => {
                match image_infos.get(raw_service_config.image()) {
                    Some(image_info) => DeployableService {
                        raw_service_config,
                        ingress_route,
                        strategy: DeploymentStrategy::RedeployOnImageUpdate(
                            image_info.digest().to_string(),
                        ),
                        declared_volumes,
                    },

                    None => DeployableService {
                        raw_service_config,
                        ingress_route,
                        strategy: DeploymentStrategy::RedeployAlways,
                        declared_volumes,
                    },
                }
            }
            crate::config::DeploymentStrategy::RedeployNever => DeployableService {
                raw_service_config,
                ingress_route,
                strategy: DeploymentStrategy::RedeployNever,
                declared_volumes,
            },
        })
    }

    fn container_type_index(container_type: &ContainerType) -> i32 {
        match container_type {
            ContainerType::ApplicationCompanion => 0,
            ContainerType::ServiceCompanion => 1,
            ContainerType::Instance | ContainerType::Replica => 2,
        }
    }
}

impl DeploymentUnitBuilder<WithAppliedTemplating> {
    pub async fn apply_hooks(
        self,
        config: &Config,
    ) -> Result<DeploymentUnitBuilder<WithAppliedHooks>, AppsServiceError> {
        let hooks = Hooks::new(config);
        let services = hooks
            .apply_deployment_hook(&self.stage.app_name, self.stage.services)
            .await?;

        Ok(DeploymentUnitBuilder {
            stage: WithAppliedHooks {
                app_name: self.stage.app_name,
                services,
            },
        })
    }
}

impl DeploymentUnitBuilder<WithAppliedHooks> {
    pub fn apply_base_traefik_ingress_route(
        mut self,
        route: TraefikIngressRoute,
    ) -> DeploymentUnitBuilder<WithAppliedIngressRoute> {
        for service in &mut self.stage.services {
            let service_route = std::mem::replace(&mut service.ingress_route, route.clone());
            service.ingress_route.merge_with(service_route);
        }

        let mut route = route;
        route.merge_with(TraefikIngressRoute::with_app_only_defaults(
            &self.stage.app_name,
        ));

        DeploymentUnitBuilder {
            stage: WithAppliedIngressRoute {
                app_name: self.stage.app_name,
                services: self.stage.services,
                route,
            },
        }
    }

    pub fn build(self) -> DeploymentUnit {
        let route = TraefikIngressRoute::with_app_only_defaults(&self.stage.app_name);
        DeploymentUnit {
            app_name: self.stage.app_name,
            services: self.stage.services,
            route,
        }
    }
}

impl DeploymentUnitBuilder<WithAppliedIngressRoute> {
    pub fn build(self) -> DeploymentUnit {
        DeploymentUnit {
            app_name: self.stage.app_name,
            services: self.stage.services,
            route: self.stage.route,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::TraefikRouterRule;
    use crate::models::{Environment, EnvironmentVariable};
    use crate::{config_from_str, sc};
    use secstr::SecUtf8;

    #[tokio::test]
    async fn should_return_unique_images() -> Result<(), AppsServiceError> {
        let config = Config::default();
        let unit = DeploymentUnitBuilder::init(
            AppName::master(),
            vec![
                sc!("http1", "nginx:1.13"),
                sc!("wordpress1", "wordpress:alpine"),
            ],
        )
        .extend_with_config(&config)
        .extend_with_templating_only_service_configs(Vec::new())
        .images();

        assert_eq!(unit.len(), 2);

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_port_mappings() -> Result<(), AppsServiceError> {
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

        let app_name = AppName::master();
        let service_configs = vec![sc!("http1", "nginx:1.13")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
        assert_eq!(configs[0].port(), 80);
        assert_eq!(configs[1].port(), 80);
        assert_eq!(configs[2].port(), 80);

        Ok(())
    }

    #[tokio::test]
    async fn should_merge_with_application_companion_if_services_contain_same_service_name(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ "VAR_1=abcd", "VAR_2=1234" ]
        "#
        );

        let app_name = AppName::master();
        let service_configs = vec![sc!(
            "openid",
            labels = (),
            env = ("VAR_1" => "efg"),
            files = ()
        )];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let openid_configs: Vec<_> = unit.services;
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

        Ok(())
    }

    #[tokio::test]
    async fn should_merge_with_service_companion_if_services_contain_same_service_name(
    ) -> Result<(), AppsServiceError> {
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

        let app_name = AppName::master();
        let service_configs = vec![sc!(
            "openid",
            labels = (),
            env = ("VAR_1" => "efg"),
            files = ()
        )];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let openid_configs: Vec<_> = unit.services;
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

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_templating_on_service_companions() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.db]
            serviceName = '{{service.name}}-db'
            type = 'service'
            image = 'postgres:11'
        "#
        );
        let app_name = AppName::master();
        let service_configs = vec![
            sc!("wordpress", "wordpress:alpine"),
            sc!("nextcloud", "nextcloud:alpine"),
        ];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
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

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_templating_on_service_environment_variables(
    ) -> Result<(), AppsServiceError> {
        let mut service_config = sc!("wordpress-db", "mariadb:10.3.17");
        service_config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_templating(
                String::from("MYSQL_DATABASE"),
                SecUtf8::from("{{service.name}}"),
            ),
        ])));

        let config = Config::default();
        let app_name = AppName::master();

        let unit = DeploymentUnitBuilder::init(app_name, vec![service_config])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
        assert_eq!(configs.len(), 1);
        let env = configs[0].env().unwrap();

        assert_eq!(
            env.variable("MYSQL_DATABASE"),
            Some(&EnvironmentVariable::new(
                String::from("MYSQL_DATABASE"),
                SecUtf8::from("wordpress-db")
            ))
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_not_apply_templating_on_service_environment_variables(
    ) -> Result<(), AppsServiceError> {
        let mut service_configs = sc!("wordpress-db", "mariadb:10.3.17");
        service_configs.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("MYSQL_DATABASE"),
            SecUtf8::from("{{application.name}}"),
        )])));

        let app_name = AppName::master();
        let config = Config::default();

        let unit = DeploymentUnitBuilder::init(app_name, vec![service_configs])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
        assert_eq!(configs.len(), 1);
        let env = configs[0].env().unwrap();

        assert_eq!(
            env.variable("MYSQL_DATABASE"),
            Some(&EnvironmentVariable::new(
                String::from("MYSQL_DATABASE"),
                SecUtf8::from("{{application.name}}")
            ))
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_templating_on_app_companions() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = '{{application.name}}-openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ """SERVICES={{~#each services~}}{{name}},{{~/each~}}""" ]
        "#
        );

        let app_name = AppName::master();
        let service_configs = vec![sc!("wordpress", "wordpress:latest")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
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

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_templating_on_app_companions_with_templating_only_configs(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = '{{application.name}}-openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ """SERVICES={{~#each services~}}{{name}},{{~/each~}}""" ]
        "#
        );

        let app_name = AppName::master();
        let service_configs = vec![sc!("wordpress", "wordpress:alpine")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(vec![sc!("postgres", "postgres:alpine")])
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
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

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_templating_on_merged_application_companions(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ "VAR_1={{application.name}}" ]
        "#
        );

        let app_name = AppName::master();
        let service_configs = vec![sc!("openid", "private.example.com/library/openid:backup")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let openid_configs: Vec<_> = unit.services;
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

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_templating_on_merged_service_companions() -> Result<(), AppsServiceError>
    {
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

        let app_name = AppName::master();
        let service_configs = vec![
            sc!("wordpress", "wordpress:alpine"),
            sc!("wordpress-db", "postgres:11-alpine"),
        ];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
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

        Ok(())
    }

    #[tokio::test]
    async fn should_determine_deployment_strategy_for_requested_service(
    ) -> Result<(), AppsServiceError> {
        let app_name = AppName::master();
        let service_configs = vec![
            sc!("wordpress", "wordpress:alpine"),
            sc!("wordpress-db", "postgres:11-alpine"),
        ];

        let config = Config::default();

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let services: Vec<_> = unit.services;
        assert_eq!(services.len(), 2);

        for services in services {
            if !matches!(services.strategy, DeploymentStrategy::RedeployAlways) {
                panic!(
                    "All services should have a recreation strategy but was {:?}.",
                    services
                );
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn should_determine_deployment_strategy_for_companions() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.db]
            serviceName = 'db'
            type = 'application'
            image = 'postgres:11'
            deploymentStrategy = 'redeploy-always'

            [companions.kafka]
            serviceName = 'kafka'
            type = 'application'
            image = 'kafka'
            deploymentStrategy = 'redeploy-always'
        "#
        );

        let app_name = AppName::master();
        let service_configs = vec![sc!("wordpress", "wordpress:alpine")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let services: Vec<_> = unit.services;
        assert_eq!(services.len(), 3);

        for service in services {
            if !matches!(service.strategy, DeploymentStrategy::RedeployAlways) {
                panic!(
                    "All services should have a recreation strategy but was {:?}.",
                    service
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn apply_base_traefik_router_rule() -> Result<(), AppsServiceError> {
        let config = config_from_str!("");

        let app_name = AppName::master();
        let service_configs = vec![sc!("wordpress")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .apply_base_traefik_ingress_route(TraefikIngressRoute::with_rule(
                TraefikRouterRule::path_prefix_rule(vec![String::from("my-path-prefix")]),
            ))
            .build();

        let service = unit.services.into_iter().next().unwrap();
        let rule = service
            .ingress_route
            .routes()
            .iter()
            .map(|r| r.rule())
            .next()
            .unwrap();

        assert_eq!(
            rule,
            &TraefikRouterRule::path_prefix_rule(["my-path-prefix", "master", "wordpress"])
        );
        assert!(matches!(
            service
                .ingress_route
                .routes()
                .iter()
                .flat_map(|r| r.middlewares().iter())
                .next(),
            Some(_)
        ));

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_rule_for_companion() -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.adminer]
            serviceName = 'adminer'
            type = 'application'
            image = 'adminer:4.8.1'

            [companions.adminer.routing]
            rule = "PathPrefix(`/{{application.name}}/adminer/sub-path`)"

            [companions.adminer.routing.additionalMiddlewares]
            headers = { 'customRequestHeaders' = { 'X-Forwarded-Prefix' =  '/{{application.name}}/adminer/sub-path' } }
            stripPrefix = { 'prefixes' = [ '/{{application.name}}/adminer/sub-path' ] }
        "#
        );

        let app_name = AppName::master();
        let service_configs = vec![sc!("http1", "nginx:1.13")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
        assert_eq!(configs[0].service_name(), "adminer");
        assert_eq!(
            configs[0].ingress_route().routes()[0].rule(),
            &TraefikRouterRule::from_str("PathPrefix(`/master/adminer/sub-path`)").unwrap()
        );
        assert_eq!(
            configs[0].ingress_route().routes()[0].middlewares(),
            &vec![
                crate::infrastructure::TraefikMiddleware {
                    name: String::from("custom-middleware-0"),
                    spec: serde_value::to_value(serde_json::json!({
                        "headers": {
                            "customRequestHeaders": {
                                "X-Forwarded-Prefix": "/master/adminer/sub-path"
                            }
                        }
                    }))
                    .unwrap()
                },
                crate::infrastructure::TraefikMiddleware {
                    name: String::from("custom-middleware-1"),
                    spec: serde_value::to_value(serde_json::json!({
                        "stripPrefix": {
                            "prefixes": [
                                "/master/adminer/sub-path"
                            ]
                        }
                    }))
                    .unwrap()
                },
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_apply_rule_for_companion_with_additional_middleware(
    ) -> Result<(), AppsServiceError> {
        let config = config_from_str!(
            r#"
            [companions.adminer]
            serviceName = 'adminer'
            type = 'application'
            image = 'adminer:4.8.1'

            [companions.adminer.routing.additionalMiddlewares]
            headers = { 'customRequestHeaders' = { 'X-Forwarded-Prefix' =  '/{{application.name}}/adminer/' } }
        "#
        );

        let app_name = AppName::master();
        let service_configs = vec![sc!("http1", "nginx:1.13")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)?
            .apply_hooks(&config)
            .await?
            .build();

        let configs: Vec<_> = unit.services;
        assert_eq!(configs[0].service_name(), "adminer");
        assert_eq!(
            configs[0].ingress_route().routes()[0].rule(),
            &TraefikRouterRule::from_str("PathPrefix(`/master/adminer/`)").unwrap()
        );
        assert_eq!(
            configs[0].ingress_route().routes()[0].middlewares(),
            &vec![
                crate::infrastructure::TraefikMiddleware {
                    name: String::from("master-adminer-middleware"),
                    spec: serde_value::to_value(serde_json::json!({
                        "stripPrefix": {
                            "prefixes": [
                                "/master/adminer/"
                            ]
                        }
                    }))
                    .unwrap()
                },
                crate::infrastructure::TraefikMiddleware {
                    name: String::from("custom-middleware-0"),
                    spec: serde_value::to_value(serde_json::json!({
                        "headers": {
                            "customRequestHeaders": {
                                "X-Forwarded-Prefix": "/master/adminer/"
                            }
                        }
                    }))
                    .unwrap()
                },
            ]
        );

        Ok(())
    }
}
