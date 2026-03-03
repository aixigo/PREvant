use crate::{
    AppName, Image, ImageInfo, Owner,
    app_blueprints::{ServiceConfig, UserDefinedParameters},
    app_deployment::{self, DeployableService, DeploymentUnit},
    app_instance::{self, ContainerType},
    templating::{
        ServiceOrServices, ServiceTemplateData, TemplateData, TemplatedClone, TemplatedCloneError,
    },
    traefik::{
        TraefikIngressRoute, TraefikIngressRouteMergeError, TraefikMiddleware, TraefikRouterRule,
    },
};
use handlebars::RenderError;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    marker::PhantomData,
    str::FromStr,
};
use url::Url;

/// This [TypeState builder](https://www.youtube.com/watch?v=pwmIQzLuYl0) is at the center of the
/// domain of PREvant: it models the transition from [`crate::app_blueprints`] to
/// [`crate::app_instance`].
pub struct AppDeploymentBuilder<Stage> {
    stage: Stage,
}

pub struct Initialized {
    pub app_name: AppName,
    pub app_name_to_replicate_from: Option<AppName>,
    pub configs: Vec<ServiceConfig>,
    pub user_defined_parameters: Option<UserDefinedParameters>,
    pub owners: HashSet<Owner>,
    phantom_data: PhantomData<()>,
}

impl AppDeploymentBuilder<Initialized> {
    pub fn init(
        app_name: AppName,
        configs: Vec<ServiceConfig>,
        user_defined_parameters: Option<UserDefinedParameters>,
    ) -> AppDeploymentBuilder<Initialized> {
        Self {
            stage: Initialized {
                app_name,
                app_name_to_replicate_from: None,
                configs,
                user_defined_parameters,
                owners: HashSet::new(),
                phantom_data: PhantomData,
            },
        }
    }

    pub fn with_app_to_replicate_from(mut self, app_name: Option<AppName>) -> Self {
        self.stage.app_name_to_replicate_from = app_name;
        self
    }

    pub fn with_owners<I>(mut self, owners: I) -> Self
    where
        I: IntoIterator<Item = Owner>,
    {
        let mut owners = HashSet::from_iter(owners);
        owners.extend(self.stage.owners.drain());
        self.stage.owners = Owner::normalize(owners);
        self
    }

    pub fn with_static_companions<I>(
        self,
        companions: I,
    ) -> AppDeploymentBuilder<WithStaticCompanions>
    where
        I: IntoIterator<Item = StaticCompanion>,
    {
        let mut service_companions = Vec::new();
        let mut app_companions = Vec::new();

        for companions in companions {
            match companions {
                StaticCompanion::ServiceCompanion {
                    blueprint_config,
                    labels,
                    deployment_strategy,
                    rule_template,
                    middleware_templates,
                    storage_strategy,
                } => service_companions.push((
                    blueprint_config,
                    labels,
                    deployment_strategy,
                    rule_template,
                    middleware_templates,
                    storage_strategy,
                )),
                StaticCompanion::ApplicationCompanion {
                    blueprint_config,
                    labels,
                    deployment_strategy,
                    rule_template,
                    middleware_templates,
                    storage_strategy,
                } => app_companions.push((
                    blueprint_config,
                    labels,
                    deployment_strategy,
                    rule_template,
                    middleware_templates,
                    storage_strategy,
                )),
            }
        }

        AppDeploymentBuilder {
            stage: WithStaticCompanions {
                initialized: self.stage,
                service_companions,
                app_companions,
            },
        }
    }

    /// A shortcut version to build an application just on the [`Initialized`] stage.
    ///
    /// This method is not intended to be used in production. It rather provides a convenience
    /// method to build [`DeploymentUnit`] in testing scenarios.
    ///
    /// ```rust
    /// use domain::{AppName, Image, blueprint_service};
    /// use domain::app_deployment::AppDeploymentBuilder;
    /// use std::str::FromStr;
    ///
    /// let app_name = AppName::from_str("latest").unwrap();
    /// let configs = vec![
    ///      blueprint_service!(
    ///          "db",
    ///          "mariadb",
    ///          env = (
    ///              "MARIADB_ROOT_PASSWORD" => "example",
    ///              "MARIADB_USER" => "example-user",
    ///              "MARIADB_PASSWORD" => "my_cool_secret",
    ///              "MARIADB_DATABASE" => "example-database"
    ///          )
    ///      ),
    ///      blueprint_service!(
    ///          "blog",
    ///          "wordpress",
    ///          env = (
    ///              "WORDPRESS_DB_HOST" => "db",
    ///              "WORDPRESS_DB_USER" => "example-user",
    ///              "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
    ///              "WORDPRESS_DB_NAME" => "example-database"
    ///          )
    ///      )
    /// ];
    ///
    /// let deployment_unit = AppDeploymentBuilder::init(app_name, configs, None)
    ///     .finish()
    ///     .unwrap();
    ///
    /// assert_eq!(deployment_unit.services.len(), 2);
    /// ```
    pub fn finish(self) -> Result<DeploymentUnit, BuildDeploymentUintBuildError> {
        let base_route = TraefikIngressRoute::empty();
        let application_base_url = base_route
            .to_url()
            .and_then(|url| url.join(&self.stage.app_name).ok());

        AppDeploymentBuilder::<WithBaseRoute> {
            stage: WithBaseRoute {
                with_resolved_images: WithResolvedImages {
                    with_resolved_apps: WithResolvedApps {
                        with_static_companions: WithStaticCompanions {
                            initialized: self.stage,
                            service_companions: Vec::new(),
                            app_companions: Vec::new(),
                        },
                        running_app: None,
                        running_app_to_replicate_from: None,
                    },
                    image_infos: HashMap::new(),
                },
                base_route,
                application_base_url,
            },
        }
        .finish()
    }
}

#[derive(Debug, PartialEq)]
pub enum StaticCompanionDeploymentStrategy {
    Always,
    OnImageUpdate,
    Never,
}

#[derive(Debug, PartialEq)]
pub enum StaticCompanionStorageStrategy {
    NoMountVolumes,
    MountDeclaredImageVolumes,
}

#[derive(Debug, PartialEq)]
pub enum StaticCompanion {
    ServiceCompanion {
        blueprint_config: ServiceConfig,
        labels: HashMap<String, String>,
        deployment_strategy: StaticCompanionDeploymentStrategy,
        rule_template: Option<String>,
        middleware_templates: Option<BTreeMap<String, serde_value::Value>>,
        storage_strategy: StaticCompanionStorageStrategy,
    },
    ApplicationCompanion {
        blueprint_config: ServiceConfig,
        labels: HashMap<String, String>,
        deployment_strategy: StaticCompanionDeploymentStrategy,
        rule_template: Option<String>,
        middleware_templates: Option<BTreeMap<String, serde_value::Value>>,
        storage_strategy: StaticCompanionStorageStrategy,
    },
}

impl StaticCompanion {
    pub fn service_companion(blueprint_config: ServiceConfig) -> Self {
        Self::ServiceCompanion {
            blueprint_config,
            labels: HashMap::new(),
            deployment_strategy: StaticCompanionDeploymentStrategy::Always,
            rule_template: None,
            middleware_templates: None,
            storage_strategy: StaticCompanionStorageStrategy::NoMountVolumes,
        }
    }

    pub fn app_companion(blueprint_config: ServiceConfig) -> Self {
        Self::ApplicationCompanion {
            blueprint_config,
            labels: HashMap::new(),
            deployment_strategy: StaticCompanionDeploymentStrategy::Always,
            rule_template: None,
            middleware_templates: None,
            storage_strategy: StaticCompanionStorageStrategy::NoMountVolumes,
        }
    }

    pub fn with_deployment_strategy(
        mut self,
        deployment_strategy: StaticCompanionDeploymentStrategy,
    ) -> Self {
        match &mut self {
            StaticCompanion::ServiceCompanion {
                deployment_strategy: ds,
                ..
            } => {
                *ds = deployment_strategy;
            }
            StaticCompanion::ApplicationCompanion {
                deployment_strategy: ds,
                ..
            } => {
                *ds = deployment_strategy;
            }
        }
        self
    }

    pub fn with_templated_rule(mut self, rule_template: Option<String>) -> Self {
        match &mut self {
            StaticCompanion::ServiceCompanion {
                rule_template: rt, ..
            } => *rt = rule_template,
            StaticCompanion::ApplicationCompanion {
                rule_template: rt, ..
            } => *rt = rule_template,
        }
        self
    }

    pub fn with_templated_middlewares(
        mut self,
        templated_middlewars: Option<BTreeMap<String, serde_value::Value>>,
    ) -> Self {
        match &mut self {
            StaticCompanion::ServiceCompanion {
                middleware_templates: mt,
                ..
            } => *mt = templated_middlewars,
            StaticCompanion::ApplicationCompanion {
                middleware_templates: mt,
                ..
            } => *mt = templated_middlewars,
        }
        self
    }

    pub fn with_storage_strategy(
        mut self,
        storage_strategy: StaticCompanionStorageStrategy,
    ) -> Self {
        match &mut self {
            StaticCompanion::ServiceCompanion {
                storage_strategy: st,
                ..
            } => *st = storage_strategy,
            StaticCompanion::ApplicationCompanion {
                storage_strategy: st,
                ..
            } => *st = storage_strategy,
        }
        self
    }

    pub fn with_labels(mut self, labels: HashMap<String, String>) -> Self {
        match &mut self {
            StaticCompanion::ServiceCompanion { labels: lb, .. } => *lb = labels,
            StaticCompanion::ApplicationCompanion { labels: lb, .. } => *lb = labels,
        }
        self
    }
}

type StaticCompanionData = (
    ServiceConfig,
    HashMap<String, String>,
    StaticCompanionDeploymentStrategy,
    Option<String>,
    Option<BTreeMap<String, serde_value::Value>>,
    StaticCompanionStorageStrategy,
);

pub struct WithStaticCompanions {
    initialized: Initialized,
    service_companions: Vec<StaticCompanionData>,
    app_companions: Vec<StaticCompanionData>,
}

#[async_trait::async_trait]
pub trait ResolveApps {
    type Error;

    async fn fetch_app(&self, app_name: AppName) -> Result<Option<app_instance::App>, Self::Error>;
}

#[async_trait::async_trait]
impl<F, E> ResolveApps for F
where
    F: Fn(AppName) -> Result<Option<app_instance::App>, E> + Send + Sync,
    E: Send + Sync + 'static,
{
    type Error = E;

    async fn fetch_app(&self, app_name: AppName) -> Result<Option<app_instance::App>, Self::Error> {
        (self)(app_name)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ResolveAppsError<E> {
    #[error("Failed to resolve app to replicate for {app_name}: {error}")]
    AppToReplicateFrom { app_name: AppName, error: E },
    #[error("Failed to fetch running app: {0}")]
    RunningApp(E),
}

impl AppDeploymentBuilder<WithStaticCompanions> {
    /// A shortcut version to build an application just on the [`WithStaticCompanions`] stage.
    ///
    /// This method is not intended to be used in production. It rather provides a convenience
    /// method to build [`DeploymentUnit`] in testing scenarios.
    pub fn finish(self) -> Result<DeploymentUnit, BuildDeploymentUintBuildError> {
        let base_route = TraefikIngressRoute::empty();
        let application_base_url = base_route
            .to_url()
            .and_then(|url| url.join(&self.stage.initialized.app_name).ok());

        AppDeploymentBuilder::<WithBaseRoute> {
            stage: WithBaseRoute {
                with_resolved_images: WithResolvedImages {
                    with_resolved_apps: WithResolvedApps {
                        with_static_companions: self.stage,
                        running_app: None,
                        running_app_to_replicate_from: None,
                    },
                    image_infos: HashMap::new(),
                },
                base_route,
                application_base_url,
            },
        }
        .finish()
    }

    /// Resolves the running applications that are defined by [`Initialized::app_name`] or
    /// [`Initialized::app_name_to_replicate_from`].
    pub async fn resolve_apps<E, R>(
        self,
        resolve_app: R,
    ) -> Result<AppDeploymentBuilder<WithResolvedApps>, ResolveAppsError<R::Error>>
    where
        R: ResolveApps,
    {
        let running_app = resolve_app
            .fetch_app(self.stage.initialized.app_name.clone())
            .await
            .map_err(ResolveAppsError::RunningApp)?;
        let running_app_to_replicate_from =
            match self.stage.initialized.app_name_to_replicate_from.as_ref() {
                Some(app_to_replicate_from) => resolve_app
                    .fetch_app(app_to_replicate_from.clone())
                    .await
                    .map_err(|error| ResolveAppsError::AppToReplicateFrom {
                        app_name: app_to_replicate_from.clone(),
                        error,
                    })?,
                None => None,
            };

        Ok(AppDeploymentBuilder {
            stage: WithResolvedApps {
                with_static_companions: self.stage,
                running_app,
                running_app_to_replicate_from,
            },
        })
    }
}
pub struct WithResolvedApps {
    with_static_companions: WithStaticCompanions,
    running_app: Option<app_instance::App>,
    running_app_to_replicate_from: Option<app_instance::App>,
}

impl WithResolvedApps {
    fn blueprint_configs_as_template_data<P: Fn(&Image) -> u16>(
        &self,
        port_mapping: P,
    ) -> Vec<ServiceTemplateData<'_>> {
        let mut template_data = HashMap::<&str, ServiceTemplateData>::new();

        for config in self.with_static_companions.initialized.configs.iter() {
            template_data.insert(
                &config.service_name,
                ServiceTemplateData {
                    name: &config.service_name,
                    image: &config.image,
                    port: port_mapping(&config.image),
                    container_type: &ContainerType::Instance,
                },
            );
        }

        for service in self
            .running_app_to_replicate_from
            .iter()
            .flat_map(|running_app| running_app.services.iter())
            .chain(
                // replicated apps take precedence over the running app
                self.running_app
                    .iter()
                    .flat_map(|running_app| running_app.services.iter()),
            )
            .filter(|service| {
                matches!(
                    service.service_type,
                    ContainerType::Instance | ContainerType::Replica
                )
            })
        {
            template_data
                .entry(service.service_name())
                .or_insert_with(|| ServiceTemplateData {
                    name: &service.blueprint_config.service_name,
                    image: &service.blueprint_config.image,
                    port: port_mapping(&service.blueprint_config.image),
                    container_type: &service.service_type,
                });
        }

        // TODO: do we have to include service companions too?

        let mut template_data = template_data.into_values().collect::<Vec<_>>();
        template_data.sort_by(|a, b| a.name.cmp(b.name));
        template_data
    }
}

impl AppDeploymentBuilder<WithResolvedApps> {
    pub fn images(&self) -> HashSet<Image> {
        let mut images = HashSet::new();

        images.extend(
            self.stage
                .with_static_companions
                .initialized
                .configs
                .iter()
                .map(|config| &config.image),
        );
        images.extend(
            self.stage
                .running_app
                .iter()
                .flat_map(|app| app.services.iter())
                .map(|running_service| &running_service.blueprint_config.image),
        );
        images.extend(
            self.stage
                .running_app_to_replicate_from
                .iter()
                .flat_map(|app| app.services.iter())
                .map(|running_service| &running_service.blueprint_config.image),
        );
        images.extend(
            self.stage
                .with_static_companions
                .app_companions
                .iter()
                .map(|(config, ..)| &config.image),
        );
        images.extend(
            self.stage
                .with_static_companions
                .service_companions
                .iter()
                .map(|(config, ..)| &config.image),
        );

        images.into_iter().cloned().collect::<HashSet<_>>()
    }

    pub async fn resolve_image_manifests<E, P>(
        self,
        resolve_images: P,
    ) -> Result<AppDeploymentBuilder<WithResolvedImages>, E>
    where
        P: AsyncFnOnce(HashSet<Image>) -> Result<HashMap<Image, ImageInfo>, E>,
    {
        let image_infos = resolve_images(self.images()).await?;
        Ok(AppDeploymentBuilder {
            stage: WithResolvedImages {
                with_resolved_apps: self.stage,
                image_infos,
            },
        })
    }
}

pub struct WithResolvedImages {
    with_resolved_apps: WithResolvedApps,
    image_infos: HashMap<Image, ImageInfo>,
}

impl AppDeploymentBuilder<WithResolvedImages> {
    pub async fn resolve_base_route<E, P>(
        self,
        resolve_base_route: P,
    ) -> Result<AppDeploymentBuilder<WithBaseRoute>, E>
    where
        P: AsyncFnOnce() -> Result<Option<TraefikIngressRoute>, E>,
    {
        let base_route = resolve_base_route()
            .await?
            .unwrap_or_else(TraefikIngressRoute::empty);

        let application_base_url = base_route.to_url().and_then(|url| {
            url.join(
                &self
                    .stage
                    .with_resolved_apps
                    .with_static_companions
                    .initialized
                    .app_name,
            )
            .ok()
        });

        Ok(AppDeploymentBuilder {
            stage: WithBaseRoute {
                with_resolved_images: self.stage,
                base_route,
                application_base_url,
            },
        })
    }
}

pub struct WithBaseRoute {
    with_resolved_images: WithResolvedImages,
    base_route: TraefikIngressRoute,
    application_base_url: Option<Url>,
}

#[derive(thiserror::Error, Debug)]
pub enum BuildDeploymentUintBuildError {
    #[error("Failed to build deployment unit: {0}")]
    TraefikIngressRouteMergeError(TraefikIngressRouteMergeError),
    #[error("Failed templating for service companion: {0}")]
    FailedTemplatingForServiceCompanions(RenderError),
    #[error("Failed templating for application companion: {0}")]
    FailedTemplatingForApplicationCompanions(RenderError),
    #[error("Failed templating for service: {0}")]
    FailedTemplatingForService(RenderError),
    #[error("Failed to parse Traefik rule from templated rule ({rule}) in static companion: {err}")]
    TraefikRuleParsingFromTemplatedStaticCompanionRule { rule: String, err: String },
}

impl From<TraefikIngressRouteMergeError> for BuildDeploymentUintBuildError {
    fn from(error: TraefikIngressRouteMergeError) -> Self {
        Self::TraefikIngressRouteMergeError(error)
    }
}

impl AppDeploymentBuilder<WithBaseRoute> {
    fn service_route(
        &self,
        service_name: &str,
    ) -> Result<TraefikIngressRoute, TraefikIngressRouteMergeError> {
        let app_name = &self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .initialized
            .app_name;

        let mut ingress_route = self.stage.base_route.clone();
        ingress_route.merge_with(TraefikIngressRoute::with_defaults(app_name, service_name))?;
        Ok(ingress_route)
    }

    fn port(&self, image: &Image) -> u16 {
        self.stage
            .with_resolved_images
            .image_infos
            .get(image)
            .and_then(|image_info| image_info.exposed_port())
            .unwrap_or(80)
    }

    fn deployment_strategy(
        &self,
        strategy: &StaticCompanionDeploymentStrategy,
        image: &Image,
    ) -> app_deployment::DeploymentStrategy {
        match strategy {
            StaticCompanionDeploymentStrategy::Always => app_deployment::DeploymentStrategy::Always,
            StaticCompanionDeploymentStrategy::OnImageUpdate => self
                .stage
                .with_resolved_images
                .image_infos
                .get(image)
                .map(|image_info| {
                    app_deployment::DeploymentStrategy::OnImageUpdate(image_info.digest.clone())
                })
                .unwrap_or_else(|| app_deployment::DeploymentStrategy::Always),
            StaticCompanionDeploymentStrategy::Never => app_deployment::DeploymentStrategy::Never,
        }
    }

    fn create_deployable_service_for_companion(
        &self,
        blueprint_service: ServiceConfig,
        origin_companion_data: &StaticCompanionData,
        template_data: &TemplateData,
        service_type: ContainerType,
    ) -> Result<DeployableService, BuildDeploymentUintBuildError> {
        let (
            _blueprint_service,
            labels,
            deployment_strategy,
            rule_template,
            middleware_templates,
            storage_strategy,
        ) = origin_companion_data;

        fn map_render_error(
            service_type: ContainerType,
            e: RenderError,
        ) -> BuildDeploymentUintBuildError {
            match service_type {
                ContainerType::ApplicationCompanion => {
                    BuildDeploymentUintBuildError::FailedTemplatingForApplicationCompanions(e)
                }
                ContainerType::ServiceCompanion => {
                    BuildDeploymentUintBuildError::FailedTemplatingForServiceCompanions(e)
                }
                service_type => {
                    unreachable!("method must not be called with {service_type}")
                }
            }
        }

        let app_name = &self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .initialized
            .app_name;
        let service_name = &blueprint_service.service_name;

        let ingress_route = if rule_template.is_some() || middleware_templates.is_some() {
            let rule = match (rule_template, middleware_templates) {
                (Some(rule_template), Some(middleware_templates)) => {
                    let rule = template_data
                        .as_handlerbars()
                        .render(rule_template)
                        .map_err(|e| map_render_error(service_type, e))?;

                    let additional_middlewares = middleware_templates
                        .iter()
                        .map(|(key, value)| {
                            Ok((
                                key.clone(),
                                template_data.as_handlerbars().render_serde_value(value)?,
                            ))
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>()
                        .map_err(|e| map_render_error(service_type, e))?;

                    let rule = TraefikRouterRule::from_str(&rule).map_err(|err|BuildDeploymentUintBuildError::TraefikRuleParsingFromTemplatedStaticCompanionRule { err, rule })?;

                    TraefikIngressRoute::with_rule_and_middlewares(
                        rule,
                        additional_middlewares
                            .into_iter()
                            .enumerate()
                            .map(|(i, (name, spec))| TraefikMiddleware {
                                name: format!("{app_name}-{service_name}-custom-middleware-{i}"),
                                spec: serde_value::to_value(serde_json::json!({
                                    name: spec.clone()
                                }))
                                .unwrap(),
                            })
                            .collect::<Vec<_>>(),
                    )
                }
                (Some(rule_template), None) => {
                    let rule = template_data
                        .as_handlerbars()
                        .render(rule_template)
                        .map_err(|e| map_render_error(service_type, e))?;

                    let rule = TraefikRouterRule::from_str(&rule).map_err(|err|BuildDeploymentUintBuildError::TraefikRuleParsingFromTemplatedStaticCompanionRule { err, rule })?;
                    TraefikIngressRoute::with_rule(rule)
                }
                (None, Some(middleware_templates)) => {
                    let additional_middlewares = middleware_templates
                        .iter()
                        .map(|(key, value)| {
                            Ok((
                                key.clone(),
                                template_data.as_handlerbars().render_serde_value(value)?,
                            ))
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>()
                        .map_err(|e| map_render_error(service_type, e))?;

                    TraefikIngressRoute::with_defaults_and_additional_middleware(
                        app_name,
                        &blueprint_service.service_name,
                        additional_middlewares
                            .into_iter()
                            .enumerate()
                            .map(|(i, (name, spec))| TraefikMiddleware {
                                name: format!("{app_name}-{service_name}-custom-middleware-{i}"),
                                spec: serde_value::to_value(serde_json::json!({
                                    name: spec.clone()
                                }))
                                .unwrap(),
                            }),
                    )
                }
                (None, None) => unreachable!(),
            };

            let mut ingress_route = self.stage.base_route.clone();
            ingress_route.merge_with(rule)?;
            ingress_route
        } else {
            self.service_route(&blueprint_service.service_name)?
        };
        let port = self.port(&blueprint_service.image);
        let strategy = self.deployment_strategy(deployment_strategy, &blueprint_service.image);

        let declared_volumes = match storage_strategy {
            StaticCompanionStorageStrategy::NoMountVolumes => Vec::new(),
            StaticCompanionStorageStrategy::MountDeclaredImageVolumes => {
                match self
                    .stage
                    .with_resolved_images
                    .image_infos
                    .get(&blueprint_service.image)
                {
                    Some(image_info) => image_info
                        .declared_volumes()
                        .iter()
                        .map(|v| (*v).clone())
                        .collect::<Vec<String>>(),
                    None => Vec::new(),
                }
            }
        };

        // TODO: we should put the standard labels from api/src/infrastructure/mod.rs
        // here
        let labels = labels
            .iter()
            .map(|(k, v)| Ok((k.clone(), template_data.as_handlerbars().render(v)?)))
            .collect::<Result<HashMap<_, _>, RenderError>>()
            .map_err(|e| map_render_error(service_type, e))?;

        Ok(DeployableService {
            blueprint_service,
            service_type,
            strategy,
            ingress_route,
            declared_volumes,
            labels,
            port,
            phantom_data: PhantomData,
        })
    }

    fn build_services_from_application_companions(
        &self,
        merged_user_defined_parameters: &Option<UserDefinedParameters>,
    ) -> Result<impl Iterator<Item = DeployableService>, BuildDeploymentUintBuildError> {
        let mut services = BTreeMap::<String, DeployableService>::new();

        let data = TemplateData {
            service_or_services: ServiceOrServices::Services {
                services: self
                    .stage
                    .with_resolved_images
                    .with_resolved_apps
                    .blueprint_configs_as_template_data(|image| self.port(image)),
            },
            ..self.base_template_data(merged_user_defined_parameters)
        };

        let instance_or_replica_configs = self
            .instances_and_replicas_iter()
            .map(|(c, _)| (c.service_name.as_str(), c))
            .collect::<HashMap<_, _>>();

        for companion_data in self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .app_companions
            .iter()
        {
            let (blueprint_service, ..) = companion_data;
            let mut blueprint_service =
                blueprint_service
                    .templated_clone(&data)
                    .map_err(|e| match e {
                        TemplatedCloneError::RenderError(e) => {
                            BuildDeploymentUintBuildError::FailedTemplatingForApplicationCompanions(
                                e,
                            )
                        }
                        TemplatedCloneError::Other(()) => {
                            unreachable!("Unit means this case in unreachable")
                        }
                    })?;

            if let Some(instance_or_replica_config) =
                instance_or_replica_configs.get(blueprint_service.service_name.as_str())
            {
                blueprint_service = ServiceConfig::clone(instance_or_replica_config)
                    .merge_with(blueprint_service.clone());

                services
                    .entry(blueprint_service.service_name.clone())
                    .or_insert(self.create_deployable_service_for_companion(
                        blueprint_service,
                        companion_data,
                        &data,
                        ContainerType::ApplicationCompanion,
                    )?);
            }
        }

        for companion_data in self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .app_companions
            .iter()
        {
            let (blueprint_service, ..) = companion_data;
            let blueprint_service =
                blueprint_service
                    .templated_clone(&data)
                    .map_err(|e| match e {
                        TemplatedCloneError::RenderError(e) => {
                            BuildDeploymentUintBuildError::FailedTemplatingForApplicationCompanions(
                                e,
                            )
                        }
                        TemplatedCloneError::Other(()) => {
                            unreachable!("Unit means this case in unreachable")
                        }
                    })?;

            services
                .entry(blueprint_service.service_name.clone())
                .or_insert(self.create_deployable_service_for_companion(
                    blueprint_service,
                    companion_data,
                    &data,
                    ContainerType::ApplicationCompanion,
                )?);
        }

        Ok(services.into_values())
    }

    fn instances_and_replicas_iter(
        &self,
    ) -> impl Iterator<Item = (&ServiceConfig, &ContainerType)> {
        // TODO: make sure that there is no overlap in service_name from the different sources
        self.stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .initialized
            .configs
            .iter()
            .map(|config| (config, &ContainerType::Instance))
            .chain(
                self.stage
                    .with_resolved_images
                    .with_resolved_apps
                    .running_app
                    .iter()
                    .flat_map(|app| app.services.iter())
                    // TODO: double check if the filtering should be done
                    .filter(|service| {
                        matches!(
                            service.service_type,
                            ContainerType::Replica | ContainerType::Instance
                        )
                    })
                    .map(|service| (&service.blueprint_config, &service.service_type)),
            )
            .chain(
                self.stage
                    .with_resolved_images
                    .with_resolved_apps
                    .running_app_to_replicate_from
                    .iter()
                    .flat_map(|app| app.services.iter())
                    // TODO: double check if the filtering should be done
                    .filter(|service| {
                        matches!(
                            service.service_type,
                            ContainerType::Replica | ContainerType::Instance
                        )
                    })
                    .map(|service| (&service.blueprint_config, &service.service_type)),
            )
    }

    fn base_template_data<'a, 'b: 'a>(
        &'b self,
        merged_user_defined_parameters: &'a Option<UserDefinedParameters>,
    ) -> TemplateData<'a> {
        TemplateData {
            application: crate::templating::ApplicationTemplateData {
                name: &self
                    .stage
                    .with_resolved_images
                    .with_resolved_apps
                    .with_static_companions
                    .initialized
                    .app_name,
                base_url: self.stage.application_base_url.as_ref(),
            },
            user_defined_parameters: merged_user_defined_parameters
                .as_ref()
                .map(|udp| udp.as_value()),
            // TODO: pass that from the infrastructure
            infrastructure: None,
            ..Default::default()
        }
    }

    fn build_services_from_service_companions(
        &self,
        merged_user_defined_parameters: &Option<UserDefinedParameters>,
    ) -> Result<Vec<DeployableService>, BuildDeploymentUintBuildError> {
        let mut deployable_services = BTreeMap::<String, DeployableService>::new();

        let mut data = self.base_template_data(merged_user_defined_parameters);

        // First pass of templating: check if the resulting companion matches to an instance or
        // replica.
        for (instance_or_replica_config, container_type) in self.instances_and_replicas_iter() {
            for companion_data in self
                .stage
                .with_resolved_images
                .with_resolved_apps
                .with_static_companions
                .service_companions
                .iter()
            {
                let (companion, ..) = companion_data;
                data.service_or_services = ServiceOrServices::Service {
                    service: ServiceTemplateData {
                        name: &instance_or_replica_config.service_name,
                        image: &instance_or_replica_config.image,
                        port: self.port(&instance_or_replica_config.image),
                        container_type,
                    },
                };

                let blueprint_service = companion.templated_clone(&data).map_err(|e| {
                    BuildDeploymentUintBuildError::FailedTemplatingForServiceCompanions(match e {
                        TemplatedCloneError::RenderError(render_error) => render_error,
                        TemplatedCloneError::Other(()) => {
                            unreachable!("Unit means this case in unreachable")
                        }
                    })
                })?;

                if blueprint_service.service_name == instance_or_replica_config.service_name {
                    let blueprint_service = instance_or_replica_config
                        .clone()
                        .merge_with(blueprint_service.clone());

                    deployable_services.insert(
                        blueprint_service.service_name.clone(),
                        self.create_deployable_service_for_companion(
                            blueprint_service,
                            companion_data,
                            &data,
                            ContainerType::ServiceCompanion,
                        )?,
                    );
                }
            }
        }

        // Second pass: now apply it for the remaining services that won't match.
        for (instance_or_replica_config, container_type) in self.instances_and_replicas_iter() {
            for companion_data in self
                .stage
                .with_resolved_images
                .with_resolved_apps
                .with_static_companions
                .service_companions
                .iter()
            {
                let (companion, ..) = companion_data;

                data.service_or_services = ServiceOrServices::Service {
                    service: ServiceTemplateData {
                        name: &instance_or_replica_config.service_name,
                        image: &instance_or_replica_config.image,
                        port: self.port(&instance_or_replica_config.image),
                        container_type,
                    },
                };

                let blueprint_service = companion.templated_clone(&data).map_err(|e| {
                    BuildDeploymentUintBuildError::FailedTemplatingForServiceCompanions(match e {
                        TemplatedCloneError::RenderError(render_error) => render_error,
                        TemplatedCloneError::Other(()) => {
                            unreachable!("Unit means this case in unreachable")
                        }
                    })
                })?;

                deployable_services
                    .entry(blueprint_service.service_name.clone())
                    .or_insert(self.create_deployable_service_for_companion(
                        blueprint_service,
                        companion_data,
                        &data,
                        ContainerType::ServiceCompanion,
                    )?);
            }
        }

        Ok(deployable_services.into_values().collect())
    }

    fn build_deployable_services_from_app(
        &self,
        app: &app_instance::App,
        service_type_override: Option<ContainerType>,
    ) -> Result<Vec<DeployableService>, TraefikIngressRouteMergeError> {
        app.services
            .iter()
            .filter(|service| {
                matches!(
                    service.service_type,
                    ContainerType::Replica | ContainerType::Instance
                )
            })
            .cloned()
            .map(|service| {
                let port = self.port(&service.blueprint_config.image);
                let ingress_route = self.service_route(&service.blueprint_config.service_name)?;

                Ok(DeployableService {
                    blueprint_service: service.blueprint_config,
                    service_type: service_type_override.unwrap_or(service.service_type),
                    // As we are filtering by ContainerType::Instance and ContainerType::Replica
                    // (see above), the service must be always deployed.
                    strategy: app_deployment::DeploymentStrategy::Always,
                    ingress_route,
                    declared_volumes: Vec::new(),
                    // TODO: we should put the standard labels from api/src/infrastructure/mod.rs
                    // here
                    labels: HashMap::new(),
                    port,
                    phantom_data: PhantomData,
                })
            })
            .collect()
    }

    fn merged_user_defined_parameters(&self) -> Option<UserDefinedParameters> {
        let user_udp = self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .initialized
            .user_defined_parameters
            .as_ref();
        let running_udp = self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .running_app
            .as_ref()
            .and_then(|app| app.user_defined_parameters.as_ref());
        let udp_to_replicate = self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .running_app_to_replicate_from
            .as_ref()
            .and_then(|app| app.user_defined_parameters.as_ref());

        match (user_udp, running_udp, udp_to_replicate) {
            (None, None, None) => None,
            (user_udp, None, None) => user_udp.cloned(),
            (None, running_udp, None) => running_udp.cloned(),
            (None, None, udp_to_replicate) => udp_to_replicate.cloned(),
            (None, Some(running_udp), Some(udp_to_replicate)) => {
                Some(running_udp.clone().merge(udp_to_replicate.clone()))
            }
            (Some(user_udp), None, Some(udp_to_replicate)) => {
                Some(udp_to_replicate.clone().merge(user_udp.clone()))
            }
            (Some(user_udp), Some(running_udp), None) => {
                Some(running_udp.clone().merge(user_udp.clone()))
            }
            (Some(user_udp), Some(running_udp), Some(udp_to_replicate)) => Some(
                running_udp
                    .clone()
                    .merge(udp_to_replicate.clone())
                    .merge(user_udp.clone()),
            ),
        }
    }

    /// This method finishes a [`DeploymentUnit`] so that it follows the rules in [the PREvant
    /// paper, see Section 4](http://dx.doi.org/10.4230/OASIcs.Microservices.2017-2019.5). For
    /// example, replication of services, applying template variables, etc.
    pub fn finish(self) -> Result<DeploymentUnit, BuildDeploymentUintBuildError> {
        let app_name = &self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .initialized
            .app_name;

        let user_defined_parameters = self.merged_user_defined_parameters();

        enum DeployableServiceOrigin {
            ServiceCompanion,
            ApplicationCompanion,
            ReplicatedFromRunningApp,
            AlreadyRunningApp,
            InitializedStage,
        }

        let mut services: HashMap<String, (DeployableService, DeployableServiceOrigin)> = self
            .build_services_from_application_companions(&user_defined_parameters)?
            .map(|deployable_service| {
                (
                    deployable_service.blueprint_service.service_name.clone(),
                    (
                        deployable_service,
                        DeployableServiceOrigin::ApplicationCompanion,
                    ),
                )
            })
            .collect::<HashMap<_, _>>();

        if let Some(running_app_to_replicate_from) = self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .running_app_to_replicate_from
            .as_ref()
        {
            // TODO: make sure that this will replace companions
            services.extend(
                self.build_deployable_services_from_app(
                    running_app_to_replicate_from,
                    Some(ContainerType::Replica),
                )?
                .into_iter()
                .map(|deployable_service| {
                    (
                        deployable_service.blueprint_service.service_name.clone(),
                        (
                            deployable_service,
                            DeployableServiceOrigin::ReplicatedFromRunningApp,
                        ),
                    )
                }),
            );
        }

        if let Some(running_app) = self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .running_app
            .as_ref()
        {
            for ds in self.build_deployable_services_from_app(running_app, None)? {
                // TODO: make sure that this will replace companions
                services
                    .entry(ds.blueprint_service.service_name.clone())
                    .and_modify(|(ods, _)| {
                        // TODO: double check that in combination with the TODO four lines above
                        //
                        // This line is triggered when we have a app_to_replicate_from and an
                        // already running application. We want to keep the service that is running
                        // as is or do we need to merge it too?
                        ods.service_type = ds.service_type;
                    })
                    .or_insert((ds, DeployableServiceOrigin::AlreadyRunningApp));
            }
        };

        let deployable_companions =
            self.build_services_from_service_companions(&user_defined_parameters)?;

        for deployable_service in deployable_companions.into_iter() {
            services
                .entry(deployable_service.blueprint_service.service_name.clone())
                .and_modify(|_| todo!())
                .or_insert((
                    deployable_service,
                    DeployableServiceOrigin::ServiceCompanion,
                ));
        }

        let template_data = self.base_template_data(&user_defined_parameters);
        for config in self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .initialized
            .configs
            .iter()
        {
            // TODO: do I have to set service?
            let config = config
                .templated_clone(&template_data)
                .map_err(|e| match e {
                    TemplatedCloneError::RenderError(render_error) => {
                        BuildDeploymentUintBuildError::FailedTemplatingForService(render_error)
                    }
                    TemplatedCloneError::Other(()) => {
                        unreachable!("Unit means this case in unreachable")
                    }
                })?;

            let mut ingress_route = self.stage.base_route.clone();
            ingress_route.merge_with(TraefikIngressRoute::with_defaults(
                app_name,
                &config.service_name,
            ))?;

            let port = self.port(&config.image);

            services
                .entry(config.service_name.clone())
                .and_modify(|(service, origin)| {
                    service.service_type = app_instance::ContainerType::Instance;

                    service.blueprint_service = match origin {
                        DeployableServiceOrigin::ApplicationCompanion
                        | DeployableServiceOrigin::ServiceCompanion
                        | DeployableServiceOrigin::ReplicatedFromRunningApp => {
                            service.blueprint_service.clone().merge_with(config.clone())
                        }
                        DeployableServiceOrigin::AlreadyRunningApp => config.clone(),
                        DeployableServiceOrigin::InitializedStage => unreachable!(
                            "Can only be reached if services with muiltiple names are passed"
                        ),
                    };
                })
                .or_insert_with(|| {
                    (
                        super::DeployableService {
                            blueprint_service: config.clone(),
                            strategy: super::DeploymentStrategy::Always,
                            service_type: app_instance::ContainerType::Instance,
                            ingress_route,
                            declared_volumes: Vec::new(),
                            // TODO: we should put the standard labels from api/src/infrastructure/mod.rs
                            // here
                            labels: HashMap::new(),
                            port,
                            phantom_data: PhantomData,
                        },
                        DeployableServiceOrigin::InitializedStage,
                    )
                });
        }

        let mut route = self.stage.base_route;
        route.merge_with(TraefikIngressRoute::with_app_only_defaults(app_name))?;

        let mut owners = self
            .stage
            .with_resolved_images
            .with_resolved_apps
            .with_static_companions
            .initialized
            .owners;
        owners.extend(
            self.stage
                .with_resolved_images
                .with_resolved_apps
                .running_app
                .as_ref()
                .iter()
                .flat_map(|app| app.owners.iter())
                .cloned(),
        );

        let mut services = services
            .into_values()
            .map(|(service, _)| service)
            .collect::<Vec<_>>();
        services.sort_by(|a, b| {
            // TODO: should we sort by service_type too??? I saw something, somewhere that indicates
            // this.
            a.blueprint_service
                .service_name
                .cmp(&b.blueprint_service.service_name)
        });

        Ok(DeploymentUnit {
            app_name: self
                .stage
                .with_resolved_images
                .with_resolved_apps
                .with_static_companions
                .initialized
                .app_name,
            services,
            route,
            user_defined_parameters,
            owners: Owner::normalize(owners),
            running_application: self
                .stage
                .with_resolved_images
                .with_resolved_apps
                .running_app,
            phantom_data: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app_deployment::{DeployableService, DeploymentStrategy},
        app_instance::{App, ContainerType},
        traefik::TraefikRouterRule,
    };
    use anyhow::Result;
    use pretty_assertions::assert_eq;

    /// A Nextcloud config which matches to [`mariadb_config`].
    fn nextcloud_config() -> ServiceConfig {
        blueprint_service!(
            "nextcloud",
            "nextcloud",
            env = (
                "MYSQL_DATABASE" => "example",
                "MYSQL_USER" => "example-user",
                "MYSQL_PASSWORD" => "my_cool_secret",
                "MYSQL_HOST" => "db"
            )
        )
    }

    fn mariadb_config_as_companion_config() -> ServiceConfig {
        let mut config = mariadb_config();
        config.service_name = String::from("{{service.name}}-db");
        config
    }

    fn mariadb_config() -> ServiceConfig {
        blueprint_service!(
            "db",
            "mariadb",
            env = (
                "MARIADB_ROOT_PASSWORD" => "example",
                "MARIADB_USER" => "example-user",
                "MARIADB_PASSWORD" => "my_cool_secret",
                "MARIADB_DATABASE" => "example-database"
            )
        )
    }

    /// A Wordpress config which matches to [`mariadb_config`].
    fn wordpress_config() -> ServiceConfig {
        blueprint_service!(
            "blog",
            "wordpress",
            env = (
                "WORDPRESS_DB_HOST" => "db",
                "WORDPRESS_DB_USER" => "example-user",
                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                "WORDPRESS_DB_NAME" => "example-database"
            )
        )
    }

    fn wordpress_configs() -> Vec<ServiceConfig> {
        vec![mariadb_config(), wordpress_config()]
    }

    fn sort_by_name(mut services: Vec<DeployableService>) -> Vec<DeployableService> {
        services.sort_by(|a, b| {
            a.blueprint_service
                .service_name
                .cmp(&b.blueprint_service.service_name)
        });

        services
    }

    #[tokio::test]
    async fn return_unique_images() -> Result<()> {
        let images = AppDeploymentBuilder::init(
            AppName::master(),
            vec![
                blueprint_service!("http1", "nginx:1.13"),
                blueprint_service!("wordpress1", "wordpress:alpine"),
            ],
            None,
        )
        .with_app_to_replicate_from(AppName::from_str("other").ok())
        .with_static_companions(vec![
            StaticCompanion::service_companion(blueprint_service!("http2", "nginx:alpine")),
            StaticCompanion::app_companion(blueprint_service!("http4", "httpd:latest")),
        ])
        .resolve_apps::<anyhow::Error, _>(|app_name| {
            Ok::<_, anyhow::Error>(Some(App::new(
                vec![app_instance::Service {
                    id: String::from("http1"),
                    service_type: ContainerType::Instance,
                    status: app_instance::ServiceStatus::Paused,
                    blueprint_config: if app_name == AppName::master() {
                        blueprint_service!("http1", "nginx:1.14")
                    } else {
                        blueprint_service!("http1", "httpd:trixie")
                    },
                }],
                HashSet::new(),
                None,
                None,
            )))
        })
        .await?
        .images();

        assert_eq!(
            images,
            HashSet::from([
                Image::from_str("nginx:1.13").unwrap(),
                Image::from_str("nginx:1.14").unwrap(),
                Image::from_str("nginx:alpine").unwrap(),
                Image::from_str("wordpress:alpine").unwrap(),
                Image::from_str("httpd:latest").unwrap(),
                Image::from_str("httpd:trixie").unwrap(),
            ])
        );

        Ok(())
    }

    #[tokio::test]
    async fn build_basic_application() -> Result<()> {
        let deployment_unit =
            AppDeploymentBuilder::init(AppName::master(), wordpress_configs(), None)
                .with_static_companions(std::iter::empty())
                .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
                .await?
                .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                .await?
                .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                .await?
                .finish()?;

        assert_eq!(
            deployment_unit,
            DeploymentUnit {
                app_name: AppName::master(),
                services: sort_by_name(
                    wordpress_configs()
                        .into_iter()
                        .map(|config| {
                            let ingress_route = TraefikIngressRoute::with_defaults(
                                &AppName::master(),
                                &config.service_name,
                            );

                            DeployableService {
                                blueprint_service: config,
                                strategy: DeploymentStrategy::Always,
                                service_type: ContainerType::Instance,
                                ingress_route,
                                declared_volumes: Vec::new(),
                                // TODO: we should put the standard labels from api/src/infrastructure/mod.rs
                                // here
                                labels: HashMap::new(),
                                port: 80,
                                phantom_data: std::marker::PhantomData,
                            }
                        })
                        .collect::<Vec<_>>()
                ),
                route: TraefikIngressRoute::with_app_only_defaults(&AppName::master()),
                user_defined_parameters: None,
                owners: HashSet::new(),
                running_application: None,
                phantom_data: PhantomData
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn build_basic_application_with_base_route() -> Result<()> {
        let deployment_unit =
            AppDeploymentBuilder::init(AppName::master(), wordpress_configs(), None)
                .with_static_companions(std::iter::empty())
                .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
                .await?
                .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                .await?
                .resolve_base_route::<anyhow::Error, _>(async || {
                    Ok(Some(TraefikIngressRoute::with_rule(
                        TraefikRouterRule::from_str("Host(`example.com`)").unwrap(),
                    )))
                })
                .await?
                .finish()?;

        assert_eq!(
            deployment_unit,
            DeploymentUnit {
                app_name: AppName::master(),
                services: sort_by_name(
                    wordpress_configs()
                        .into_iter()
                        .map(|config| {
                            let mut ingress_route = TraefikIngressRoute::with_rule(
                                TraefikRouterRule::from_str("Host(`example.com`)").unwrap(),
                            );

                            ingress_route
                                .merge_with(TraefikIngressRoute::with_defaults(
                                    &AppName::master(),
                                    &config.service_name,
                                ))
                                .unwrap();

                            DeployableService {
                                blueprint_service: config,
                                strategy: DeploymentStrategy::Always,
                                service_type: ContainerType::Instance,
                                ingress_route,
                                declared_volumes: Vec::new(),
                                // TODO: we should put the standard labels from api/src/infrastructure/mod.rs
                                // here
                                labels: HashMap::new(),
                                port: 80,
                                phantom_data: std::marker::PhantomData,
                            }
                        })
                        .collect::<Vec<_>>()
                ),
                route: {
                    let mut route = TraefikIngressRoute::with_rule(
                        TraefikRouterRule::from_str("Host(`example.com`)").unwrap(),
                    );
                    route
                        .merge_with(TraefikIngressRoute::with_app_only_defaults(
                            &AppName::master(),
                        ))
                        .unwrap();
                    route
                },
                user_defined_parameters: None,
                owners: HashSet::new(),
                running_application: None,
                phantom_data: PhantomData
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn apply_port_mapping() -> Result<()> {
        let deployment_unit =
            AppDeploymentBuilder::init(AppName::master(), wordpress_configs(), None)
                .with_static_companions(std::iter::empty())
                .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
                .await?
                .resolve_image_manifests::<anyhow::Error, _>(async |_images| {
                    // we simulate that the container registry resolved the manifest info.
                    Ok(HashMap::from([(
                        Image::from_str("mariadb").unwrap(),
                        ImageInfo::with_exposed_port(3306),
                    )]))
                })
                .await?
                .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                .await?
                .finish()?;

        assert_eq!(
            deployment_unit.services.into_iter().find_map(|service| {
                if service.blueprint_service.service_name == "db" {
                    Some(service.port)
                } else {
                    None
                }
            }),
            Some(3306)
        );

        Ok(())
    }

    mod owners {
        use super::*;
        use openidconnect::{IssuerUrl, SubjectIdentifier};
        use pretty_assertions::assert_eq;

        #[tokio::test]
        async fn merge_with_running_application() -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config(), nextcloud_config()],
                None,
            )
            .with_owners(std::iter::once(Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: None,
            }))
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|app_name| {
                if app_name == AppName::master() {
                    return Ok(Some(App::new(
                        vec![app_instance::Service {
                            id: String::from("id"),
                            status: app_instance::ServiceStatus::Paused,
                            service_type: ContainerType::Replica,
                            blueprint_config: blueprint_service!("nginx", "nginx:latest"),
                        }],
                        HashSet::from([Owner {
                            sub: SubjectIdentifier::new(String::from("github-user")),
                            iss: IssuerUrl::new(String::from("https://github.com")).unwrap(),
                            name: None,
                        }]),
                        None,
                        None,
                    )));
                }
                Ok::<_, anyhow::Error>(None)
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                deployment_unit.owners,
                HashSet::from([
                    Owner {
                        sub: SubjectIdentifier::new(String::from("gitlab-user")),
                        iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                        name: None,
                    },
                    Owner {
                        sub: SubjectIdentifier::new(String::from("github-user")),
                        iss: IssuerUrl::new(String::from("https://github.com")).unwrap(),
                        name: None,
                    }
                ])
            );

            Ok(())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::identical(
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },
        )]
        #[case::new_has_no_name(
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: None
            },
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },
        )]
        #[case::existing_has_no_name(
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: None
            },
        )]
        #[case::both_have_names(
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("user_login")),
            },
        )]
        #[case::both_have_names(
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
        )]
        async fn merge_with_running_application_and_normalize(
            #[case] new_owner: Owner,
            #[case] existing_owner: Owner,
        ) -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config(), nextcloud_config()],
                None,
            )
            .with_owners(std::iter::once(new_owner))
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|app_name| {
                if app_name == AppName::master() {
                    return Ok(Some(App::new(
                        vec![app_instance::Service {
                            id: String::from("id"),
                            status: app_instance::ServiceStatus::Paused,
                            service_type: ContainerType::Replica,
                            blueprint_config: blueprint_service!("nginx", "nginx:latest"),
                        }],
                        HashSet::from([existing_owner.clone()]),
                        None,
                        None,
                    )));
                }
                Ok::<_, anyhow::Error>(None)
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                deployment_unit.owners,
                HashSet::from([Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: Some(String::from("Some Person")),
                }])
            );

            Ok(())
        }

        #[tokio::test]
        async fn not_merge_with_replicated_application() -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config(), nextcloud_config()],
                None,
            )
            .with_app_to_replicate_from(Some(AppName::from_str("other").unwrap()))
            .with_owners(std::iter::once(Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: None,
            }))
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|app_name| {
                if app_name != AppName::master() {
                    return Ok(Some(App::new(
                        vec![app_instance::Service {
                            id: String::from("id"),
                            status: app_instance::ServiceStatus::Paused,
                            service_type: ContainerType::Replica,
                            blueprint_config: blueprint_service!("nginx", "nginx:latest"),
                        }],
                        HashSet::from([Owner {
                            sub: SubjectIdentifier::new(String::from("gitlab-user")),
                            iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                            name: None,
                        }]),
                        None,
                        None,
                    )));
                }
                Ok::<_, anyhow::Error>(None)
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                deployment_unit.owners,
                HashSet::from([Owner {
                    sub: SubjectIdentifier::new(String::from("gitlab-user")),
                    iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                    name: None,
                },])
            );

            Ok(())
        }
    }

    mod companions {
        use super::*;
        use pretty_assertions::assert_eq;

        #[tokio::test]
        async fn service_companion() -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config(), nextcloud_config()],
                None,
            )
            .with_static_companions(vec![StaticCompanion::service_companion(
                mariadb_config_as_companion_config(),
            )])
            .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "blog-db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::ServiceCompanion,
                        &blueprint_service!(
                            "blog-db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database"
                            )
                        ),
                    ),
                    (
                        "nextcloud",
                        &Image::from_str("nextcloud").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "nextcloud",
                            "nextcloud",
                            env = (
                                "MYSQL_DATABASE" => "example",
                                "MYSQL_USER" => "example-user",
                                "MYSQL_PASSWORD" => "my_cool_secret",
                                "MYSQL_HOST" => "db"
                            )
                        ),
                    ),
                    (
                        "nextcloud-db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::ServiceCompanion,
                        &blueprint_service!(
                            "nextcloud-db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[test]
        fn app_companion() -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), vec![wordpress_config()], None)
                    .with_static_companions(vec![StaticCompanion::app_companion(mariadb_config())])
                    .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::ApplicationCompanion,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[rstest::rstest]
        #[case::app_companions(vec![
            StaticCompanion::app_companion(
                blueprint_service!(
                  "blog",
                  "wordpress",
                  env = (
                      "WORDPRESS_TABLE_PREFIX" => "test_",
                      "WORDPRESS_DB_USER" => "will-be-overwritten"
                  )
                ),
            ),
            StaticCompanion::app_companion(
                blueprint_service!(
                  "db",
                  "mariadb",
                  env = (
                      "MARIADB_AUTO_UPGRADE" => "true",
                      "MARIADB_USER" => "will-be-overwritten"
                  )
                ),
            ),
        ])]
        #[case::service_companions(vec![
            StaticCompanion::app_companion(
                blueprint_service!(
                  "blog",
                  "wordpress",
                  env = (
                      "WORDPRESS_TABLE_PREFIX" => "test_",
                      "WORDPRESS_DB_USER" => "will-be-overwritten"
                  )
                ),
            ),
            StaticCompanion::service_companion(
                blueprint_service!(
                  "db",
                  "mariadb",
                  env = (
                      "MARIADB_AUTO_UPGRADE" => "true",
                      "MARIADB_USER" => "will-be-overwritten"
                  )
                ),
            ),
        ])]
        fn merge_blueprint_and_companions(
            #[case] static_companions: Vec<StaticCompanion>,
        ) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), wordpress_configs(), None)
                    .with_static_companions(static_companions)
                    .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_TABLE_PREFIX" => "test_",
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_AUTO_UPGRADE" => "true",
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::app_companions(vec![
            StaticCompanion::app_companion(blueprint_service!("db", "mariadb"))
                .with_deployment_strategy(StaticCompanionDeploymentStrategy::OnImageUpdate),
            ],
            crate::app_deployment::DeploymentStrategy::OnImageUpdate(String::from(
                    "sha256:72dd6e9f556b475090e05fcb0a9e012a89c076baa7a577e80388ea65f315edf1"
            ))
        )]
        #[case::service_companions(vec![
            StaticCompanion::service_companion(blueprint_service!("db", "mariadb"))
                .with_deployment_strategy(StaticCompanionDeploymentStrategy::OnImageUpdate),
            ],
            crate::app_deployment::DeploymentStrategy::OnImageUpdate(String::from(
                    "sha256:72dd6e9f556b475090e05fcb0a9e012a89c076baa7a577e80388ea65f315edf1"
            ))
        )]
        #[case::app_companions(vec![
            StaticCompanion::app_companion(blueprint_service!("db", "mariadb"))
                .with_deployment_strategy(StaticCompanionDeploymentStrategy::Never),
            ],
            crate::app_deployment::DeploymentStrategy::Never
        )]
        #[case::service_companions(vec![
            StaticCompanion::service_companion(blueprint_service!("db", "mariadb"))
                .with_deployment_strategy(StaticCompanionDeploymentStrategy::Never),
            ],
            crate::app_deployment::DeploymentStrategy::Never
        )]
        async fn keep_deployment_strategy(
            #[case] static_companions: Vec<StaticCompanion>,
            #[case] expected_deployment_strategy: crate::app_deployment::DeploymentStrategy,
        ) -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config()],
                None,
            )
            .with_static_companions(static_companions)
            .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| {
                Ok(HashMap::from([(
                    Image::from_str("mariadb").unwrap(),
                    ImageInfo::with_image_digest(String::from(
                        "sha256:72dd6e9f556b475090e05fcb0a9e012a89c076baa7a577e80388ea65f315edf1",
                    )),
                )]))
            })
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                vec![(
                    "db",
                    &Image::from_str("mariadb").unwrap(),
                    &blueprint_service!("db", "mariadb"),
                    &expected_deployment_strategy
                ),],
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "db")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.blueprint_service,
                        &service.strategy
                    ))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }

        #[rstest::rstest]
        #[case::app_companions(vec![
            StaticCompanion::app_companion(blueprint_service!("db", "mariadb"))
                .with_deployment_strategy(StaticCompanionDeploymentStrategy::OnImageUpdate),
        ])]
        #[case::service_companions(vec![
            StaticCompanion::service_companion(blueprint_service!("db", "mariadb"))
                .with_deployment_strategy(StaticCompanionDeploymentStrategy::OnImageUpdate),
        ])]
        fn use_strategy_always_if_image_info_is_unavailable(
            #[case] static_companions: Vec<StaticCompanion>,
        ) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), vec![wordpress_config()], None)
                    .with_static_companions(static_companions)
                    .finish()?;

            assert_eq!(
                vec![(
                    "db",
                    &Image::from_str("mariadb").unwrap(),
                    &blueprint_service!("db", "mariadb"),
                    &crate::app_deployment::DeploymentStrategy::Always
                ),],
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "db")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.blueprint_service,
                        &service.strategy
                    ))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::app_companions_mounting_the_volume(
            vec![
                StaticCompanion::app_companion(blueprint_service!("db", "mariadb"))
                    .with_storage_strategy(StaticCompanionStorageStrategy::MountDeclaredImageVolumes),
            ],
            vec![String::from("/var/lib/mysql")],
        )]
        #[case::app_companions_not_mounting_the_volume(
            vec![
                StaticCompanion::app_companion(blueprint_service!("db", "mariadb"))
            ],
            vec![],
        )]
        #[case::service_companions_mounting_the_volume(
            vec![
                StaticCompanion::service_companion(blueprint_service!("db", "mariadb"))
                    .with_storage_strategy(StaticCompanionStorageStrategy::MountDeclaredImageVolumes),
            ],
            vec![String::from("/var/lib/mysql")],
        )]
        #[case::service_companions_not_mounting_the_volume(
            vec![
                StaticCompanion::service_companion(blueprint_service!("db", "mariadb"))
            ],
            vec![],
        )]
        async fn initialize_declared_volumes(
            #[case] static_companions: Vec<StaticCompanion>,
            #[case] expected_declared_volumes: Vec<String>,
        ) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), vec![wordpress_config()], None)
                    .with_static_companions(static_companions)
                    .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
                    .await?
                    .resolve_image_manifests::<anyhow::Error, _>(async |_images| {
                        Ok(HashMap::from([(
                            Image::from_str("mariadb").unwrap(),
                            ImageInfo::with_declared_volumes(vec![String::from("/var/lib/mysql")]),
                        )]))
                    })
                    .await?
                    .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                    .await?
                    .finish()?;

            assert_eq!(
                vec![("db", &expected_declared_volumes),],
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "db")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.declared_volumes
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::app_companions_mounting_the_volume(
            vec![
                StaticCompanion::app_companion(blueprint_service!("db", "mariadb"))
                    .with_labels(HashMap::from([(String::from("com.github.prevant"), String::from("bar-{{application.name}}"))])),
            ],
            HashMap::from([(String::from("com.github.prevant"), String::from("bar-master"))]),
        )]
        #[case::app_companions_not_mounting_the_volume(
            vec![
                StaticCompanion::app_companion(blueprint_service!("db", "mariadb"))
            ],
            HashMap::new(),
        )]
        #[case::service_companions_mounting_the_volume(
            vec![
                StaticCompanion::service_companion(blueprint_service!("db", "mariadb"))
                    .with_labels(HashMap::from([(String::from("com.github.prevant"), String::from("bar-{{application.name}}"))])),
            ],
            HashMap::from([(String::from("com.github.prevant"), String::from("bar-master"))]),
        )]
        #[case::service_companions_not_mounting_the_volume(
            vec![
                StaticCompanion::service_companion(blueprint_service!("db", "mariadb"))
            ],
            HashMap::new(),
        )]
        async fn initialize_lables(
            #[case] static_companions: Vec<StaticCompanion>,
            #[case] expected_lables: HashMap<String, String>,
        ) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), vec![wordpress_config()], None)
                    .with_static_companions(static_companions)
                    .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
                    .await?
                    .resolve_image_manifests::<anyhow::Error, _>(async |_images| {
                        Ok(HashMap::from([(
                            Image::from_str("mariadb").unwrap(),
                            ImageInfo::with_declared_volumes(vec![String::from("/var/lib/mysql")]),
                        )]))
                    })
                    .await?
                    .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                    .await?
                    .finish()?;

            assert_eq!(
                vec![("db", &expected_lables),],
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "db")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.labels
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }
    }

    mod replication {
        use super::*;
        use chrono::Utc;
        use pretty_assertions::assert_eq;

        #[rstest::rstest]
        #[case::replicate_instance(ContainerType::Instance)]
        #[case::replicate_replica_too(ContainerType::Replica)]
        #[tokio::test]
        async fn replicate_database_from_master(
            #[case] database_service_type: ContainerType,
        ) -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::from_str("other").unwrap(),
                vec![wordpress_config()],
                None,
            )
            .with_app_to_replicate_from(Some(AppName::master()))
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(move |app_name| {
                if app_name == AppName::master() {
                    return Ok(Some(App::new(
                        vec![app_instance::Service {
                            id: String::from("id"),
                            status: app_instance::ServiceStatus::Running {
                                started_at: Utc::now(),
                            },
                            service_type: database_service_type,
                            blueprint_config: mariadb_config(),
                        }],
                        HashSet::new(),
                        None,
                        None,
                    )));
                }
                Ok::<_, anyhow::Error>(None)
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Replica,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[tokio::test]
        async fn overwrite_service_type_if_replica_is_also_contained_in_payload() -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::from_str("other").unwrap(),
                wordpress_configs(),
                None,
            )
            .with_app_to_replicate_from(Some(AppName::master()))
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|app_name| {
                if app_name == AppName::master() {
                    Ok(Some(App::new(
                        vec![app_instance::Service {
                            id: String::from("id"),
                            status: app_instance::ServiceStatus::Running {
                                started_at: Utc::now(),
                            },
                            service_type: ContainerType::Instance,
                            blueprint_config: blueprint_service!(mariadb_config(), env = (
                                    "MARIADB_AUTO_UPGRADE" => "true"
                            )),
                        }],
                        HashSet::new(),
                        None,
                        None,
                    )))
                } else {
                    Ok::<_, anyhow::Error>(None)
                }
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database",
                                "MARIADB_AUTO_UPGRADE" => "true"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[tokio::test]
        async fn must_not_replicate_companions() -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::from_str("other").unwrap(), vec![wordpress_config()], None
            )
            .with_app_to_replicate_from(Some(AppName::master()))
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|app_name| {
                if app_name == AppName::master() {
                    Ok(Some(App::new(
                        vec![app_instance::Service {
                            id: String::from("id-1"),
                            status: app_instance::ServiceStatus::Running {
                                started_at: Utc::now(),
                            },
                            service_type: ContainerType::ApplicationCompanion,
                            blueprint_config: blueprint_service!(mariadb_config(), env = ("TEST" => "test")),
                        }, app_instance::Service {
                            id: String::from("id-2"),
                            status: app_instance::ServiceStatus::Running {
                                started_at: Utc::now(),
                            },
                            service_type: ContainerType::ServiceCompanion,
                            blueprint_config: blueprint_service!("api", "gateway:example"),
                        }],
                        HashSet::new(),
                        None,
                        None,
                    )))
                } else {
                    Ok::<_, anyhow::Error>(None)
                }
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                vec![(
                    "blog",
                    &Image::from_str("wordpress").unwrap(),
                    &ContainerType::Instance,
                    &blueprint_service!(
                        "blog",
                        "wordpress",
                        env = (
                            "WORDPRESS_DB_HOST" => "db",
                            "WORDPRESS_DB_USER" => "example-user",
                            "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                            "WORDPRESS_DB_NAME" => "example-database"
                        )
                    ),
                )],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[tokio::test]
        async fn running_services_of_replicated_but_running_app_take_precedence_over_second_replicas()
        -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::from_str("other").unwrap(),
                wordpress_configs(),
                None,
            )
            .with_app_to_replicate_from(Some(AppName::master()))
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|app_name: AppName| {
                Ok::<_, anyhow::Error>(Some(App::new(
                    vec![app_instance::Service {
                        id: String::from("id"),
                        status: app_instance::ServiceStatus::Running {
                            started_at: Utc::now(),
                        },
                        service_type: ContainerType::Instance,
                        blueprint_config: blueprint_service!(mariadb_config(), env = (
                                "MARIADB_AUTO_UPGRADE" => match app_name.as_str() {
                                    "other" => "false",
                                    _ => "true"
                                }
                        )),
                    }],
                    HashSet::new(),
                    None,
                    None,
                )))
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database",
                                "MARIADB_AUTO_UPGRADE" => "true"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[tokio::test]
        async fn instances_of_running_apps_remain() -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::from_str("other").unwrap(),
                vec![wordpress_config()],
                None,
            )
            .with_app_to_replicate_from(Some(AppName::master()))
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|app_name: AppName| {
                Ok::<_, anyhow::Error>(Some(App::new(
                    vec![
                        app_instance::Service {
                            id: String::from("id"),
                            status: app_instance::ServiceStatus::Running {
                                started_at: Utc::now(),
                            },
                            service_type: ContainerType::Instance,
                            blueprint_config: blueprint_service!(mariadb_config(), env = (
                                    "MARIADB_AUTO_UPGRADE" => match app_name.as_str() {
                                        "other" => "false",
                                        _ => "true"
                                    }
                            )),
                        },
                        app_instance::Service {
                            id: String::from("id-2"),
                            status: app_instance::ServiceStatus::Running {
                                started_at: Utc::now(),
                            },
                            service_type: ContainerType::Instance,
                            blueprint_config: wordpress_config(),
                        },
                    ],
                    HashSet::new(),
                    None,
                    None,
                )))
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database",
                                "MARIADB_AUTO_UPGRADE" => "true"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }

        #[tokio::test]
        async fn replicate_running_application_with_companions() -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::from_str("other").unwrap(), vec![], None)
                    .with_app_to_replicate_from(Some(AppName::master()))
                    .with_static_companions(std::iter::once(StaticCompanion::service_companion(
                        blueprint_service!("db1-{{userDefined.name}}", "postgres:16.1"),
                    )))
                    .resolve_apps::<anyhow::Error, _>(|app_name| {
                        if app_name != AppName::master() {
                            return Ok(None);
                        }
                        Ok::<_, anyhow::Error>(Some(App::new(
                            vec![
                                app_instance::Service {
                                    id: String::from("id"),
                                    status: app_instance::ServiceStatus::Running {
                                        started_at: Utc::now(),
                                    },
                                    service_type: ContainerType::ServiceCompanion,
                                    blueprint_config: blueprint_service!(
                                        "db1-my-name",
                                        "postgres:16.1"
                                    ),
                                },
                                app_instance::Service {
                                    id: String::from("id-2"),
                                    status: app_instance::ServiceStatus::Running {
                                        started_at: Utc::now(),
                                    },
                                    service_type: ContainerType::Instance,
                                    blueprint_config: wordpress_config(),
                                },
                            ],
                            HashSet::new(),
                            Some(unsafe {
                                UserDefinedParameters::without_validation(serde_json::json!({
                                    "name": "my-name"
                                }))
                            }),
                            None,
                        )))
                    })
                    .await?
                    .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                    .await?
                    .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                    .await?
                    .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Replica,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "db1-my-name",
                        &Image::from_str("postgres:16.1").unwrap(),
                        &ContainerType::ServiceCompanion,
                        &blueprint_service!("db1-my-name", "postgres:16.1"),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }
    }

    mod merge_with_running_application {
        use super::*;
        use pretty_assertions::assert_eq;

        #[tokio::test]
        async fn additional_service() -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), wordpress_configs(), None)
                    .with_static_companions(std::iter::empty())
                    .resolve_apps::<anyhow::Error, _>(|_app_name| {
                        Ok::<_, anyhow::Error>(Some(App::new(
                            vec![app_instance::Service {
                                id: String::from("id"),
                                status: app_instance::ServiceStatus::Paused,
                                service_type: ContainerType::Replica,
                                blueprint_config: blueprint_service!("nginx", "nginx:latest"),
                            }],
                            HashSet::new(),
                            None,
                            None,
                        )))
                    })
                    .await?
                    .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                    .await?
                    .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                    .await?
                    .finish()?;

            assert_eq!(
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Instance
                    ),
                    (
                        "nginx",
                        &Image::from_str("nginx:latest").unwrap(),
                        &ContainerType::Replica
                    ),
                ]
            );

            Ok(())
        }

        #[tokio::test]
        async fn merge_running_services() -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), wordpress_configs(), None)
                    .with_static_companions(std::iter::empty())
                    .resolve_apps::<anyhow::Error, _>(|_app_name| {
                        Ok::<_, anyhow::Error>(Some(App::new(
                            vec![app_instance::Service {
                                id: String::from("id"),
                                status: app_instance::ServiceStatus::Paused,
                                service_type: ContainerType::Replica,
                                blueprint_config: blueprint_service!(
                                    "blog",
                                    "wordpress",
                                    env = (
                                        "WORDPRESS_TABLE_PREFIX" => "test_"
                                    )
                                ),
                            }],
                            HashSet::new(),
                            None,
                            None,
                        )))
                    })
                    .await?
                    .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                    .await?
                    .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                    .await?
                    .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "blog",
                            "wordpress",
                            env = (
                                "WORDPRESS_DB_HOST" => "db",
                                "WORDPRESS_DB_USER" => "example-user",
                                "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                "WORDPRESS_DB_NAME" => "example-database"
                            )
                        ),
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }
    }

    mod templating {
        use super::*;
        use crate::{
            app_blueprints::EnvironmentVariable, templating::Templated, traefik::TraefikMiddleware,
        };
        use pretty_assertions::assert_eq;
        use secstr::SecUtf8;
        use url::Url;

        #[tokio::test]
        async fn do_not_apply_templating_if_env_vars_are_not_marked_as_templatable() -> Result<()> {
            let mut wordpress_config = wordpress_config();
            wordpress_config.add_env(
                EnvironmentVariable::new(
                    String::from("WORDPRESS_CONFIG_EXTRA"),
                    SecUtf8::from_str("define('WP_HOME','http://localhost');\ndefine('WP_SITEURL','{{application.baseUrl}}/blog');").unwrap(),
                ),
            );

            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config, mariadb_config()],
                None,
            )
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                Some((
                    "blog",
                    &Image::from_str("wordpress").unwrap(),
                    &ContainerType::Instance,
                    &blueprint_service!(
                        "blog",
                        "wordpress",
                        env = (
                            "WORDPRESS_DB_HOST" => "db",
                            "WORDPRESS_DB_USER" => "example-user",
                            "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                            "WORDPRESS_DB_NAME" => "example-database",
                            "WORDPRESS_CONFIG_EXTRA" => "define('WP_HOME','http://localhost');\ndefine('WP_SITEURL','{{application.baseUrl}}/blog');"
                        )
                    )
                )),
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "blog")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .next(),
            );

            Ok(())
        }

        #[tokio::test]
        async fn service_config_env_vars() -> Result<()> {
            let mut wordpress_config = wordpress_config();
            wordpress_config.add_env(
                EnvironmentVariable::with_templating(
                    String::from("WORDPRESS_CONFIG_EXTRA"),
                    SecUtf8::from_str("define('WP_HOME','http://localhost');\ndefine('WP_SITEURL','{{application.baseUrl}}/blog');").unwrap(),
                ),
            );

            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config, mariadb_config()],
                None,
            )
            .with_static_companions(std::iter::empty())
            .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || {
                Ok(Some(TraefikIngressRoute::with_rule(
                    TraefikRouterRule::from_str("Host(`example.com`)").unwrap(),
                )))
            })
            .await?
            .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &{
                            let mut expected_wordpress_config = blueprint_service!(
                                "blog",
                                "wordpress",
                                env = (
                                    "WORDPRESS_DB_HOST" => "db",
                                    "WORDPRESS_DB_USER" => "example-user",
                                    "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                    "WORDPRESS_DB_NAME" => "example-database"
                                )
                            );
                            expected_wordpress_config.add_env(
                                EnvironmentVariable::with_templating(
                                    String::from("WORDPRESS_CONFIG_EXTRA"),
                                    SecUtf8::from_str("define('WP_HOME','http://localhost');\ndefine('WP_SITEURL','{{application.baseUrl}}/blog');").unwrap(),
                                ),
                            );
                            expected_wordpress_config
                                .apply_template(&TemplateData {
                                    application: crate::templating::ApplicationTemplateData {
                                        name: "master",
                                        base_url: Url::from_str("http://example.com/master")
                                            .ok()
                                            .as_ref(),
                                    },
                                    ..Default::default()
                                })
                                .unwrap()
                        },
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::app_companions(vec![
            StaticCompanion::app_companion({
                let mut openid_config = blueprint_service!(
                    "openid",
                    "private.example.com/library/openid:latest"
                );
                openid_config.add_env(
                    EnvironmentVariable::with_templating(
                        String::from("REDIRECT_URI"),
                        SecUtf8::from_str("{{application.baseUrl}}").unwrap(),
                    ),
                );
                openid_config
            }),
        ])]
        #[case::service_companions(vec![
            StaticCompanion::service_companion({
                let mut openid_config = blueprint_service!(
                    "openid",
                    "private.example.com/library/openid:latest"
                );
                openid_config.add_env(
                    EnvironmentVariable::with_templating(
                        String::from("REDIRECT_URI"),
                        SecUtf8::from_str("{{application.baseUrl}}").unwrap(),
                    ),
                );
                openid_config
            }),
        ])]
        async fn service_config_env_vars_of_merge_companion(
            #[case] static_companions: Vec<StaticCompanion>,
        ) -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![blueprint_service!(
                    "openid",
                    "private.example.com/library/openid:backup"
                )],
                None,
            )
            .with_static_companions(static_companions)
            .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || {
                Ok(Some(TraefikIngressRoute::with_rule(
                    TraefikRouterRule::from_str("Host(`example.com`)").unwrap(),
                )))
            })
            .await?
            .finish()?;

            assert_eq!(
                vec![(
                    "openid",
                    &Image::from_str("private.example.com/library/openid:backup").unwrap(),
                    &ContainerType::Instance,
                    &blueprint_service!(
                        "openid",
                        "private.example.com/library/openid:backup",
                        env = (
                            "REDIRECT_URI" => "http://example.com/master"
                        )
                    ),
                ),],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::services_are_provided_from_user(async {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config(), nextcloud_config()],
                None,
            )
            .with_static_companions(
                vec![StaticCompanion::app_companion(
                    blueprint_service!(
                        mariadb_config(),
                        env = (),
                        files = (
                            "/docker-entrypoint-initdb.d/databases-backup.sql" => r#"
                                  {{~#each services~}}
                                  CREATE DATABASE `{{name}}`;
                                  {{~/each~}}
                            "#
                        )
                    ),
                )]
            )
            .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;
            Ok(deployment_unit)
        })]
        #[case::services_are_provided_from_running_application(async {
            let deployment_unit = AppDeploymentBuilder::init(AppName::master(), Vec::new(), None)
                .with_static_companions(
                    vec![StaticCompanion::app_companion(
                        blueprint_service!(
                            mariadb_config(),
                            env = (),
                            files = (
                                "/docker-entrypoint-initdb.d/databases-backup.sql" => r#"
                                      {{~#each services~}}
                                      CREATE DATABASE `{{name}}`;
                                      {{~/each~}}
                                "#
                            )
                        ),
                    )]
                )
                .resolve_apps::<anyhow::Error, _>(|app_name| {
                    if app_name != AppName::master() {
                        return Ok::<_, anyhow::Error>(None);
                    }

                    Ok(Some(App::new(
                        vec![wordpress_config(), nextcloud_config()]
                            .into_iter()
                            .enumerate()
                            .map(|(i, blueprint_config)| app_instance::Service {
                                id: i.to_string(),
                                status: app_instance::ServiceStatus::Paused,
                                service_type: ContainerType::Instance,
                                blueprint_config,
                            })
                            .chain(std::iter::once(app_instance::Service {
                                id: String::from("db"),
                                status: app_instance::ServiceStatus::Paused,
                                service_type: ContainerType::ApplicationCompanion,
                                blueprint_config: mariadb_config(),
                            }))
                            .collect::<Vec<_>>(),
                        HashSet::new(),
                        None,
                        None,
                    )))
                })
                .await?
                .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                .await?
                .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                .await?
                .finish()?;
            Ok(deployment_unit)
        })]
        #[case::services_are_provided_from_replicated_application(async {
            let deployment_unit = AppDeploymentBuilder::init(AppName::master(), Vec::new(), None)
                .with_static_companions(
                    vec![StaticCompanion::app_companion(
                        blueprint_service!(
                            mariadb_config(),
                            env = (),
                            files = (
                                "/docker-entrypoint-initdb.d/databases-backup.sql" => r#"
                                  {{~#each services~}}
                                  CREATE DATABASE `{{name}}`;
                                  {{~/each~}}"#
                            )
                        ),
                    )]
                )
                .resolve_apps::<anyhow::Error, _>(|app_name| {
                    if app_name != AppName::master() {
                        return Ok::<_, anyhow::Error>(None);
                    }

                    Ok(Some(App::new(
                        vec![wordpress_config(), nextcloud_config()]
                            .into_iter()
                            .enumerate()
                            .map(|(i, blueprint_config)| app_instance::Service {
                                id: i.to_string(),
                                status: app_instance::ServiceStatus::Paused,
                                service_type: ContainerType::Instance,
                                blueprint_config,
                            })
                            .chain(std::iter::once(app_instance::Service {
                                id: String::from("db"),
                                status: app_instance::ServiceStatus::Paused,
                                service_type: ContainerType::ApplicationCompanion,
                                blueprint_config: mariadb_config(),
                            }))
                            .collect::<Vec<_>>(),
                        HashSet::new(),
                        None,
                        None,
                    )))
                })
                .await?
                .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                .await?
                .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                .await?
                .finish()?;
            Ok(deployment_unit)
        })]
        async fn use_services_in_application_companions_templating(
            #[future]
            #[case]
            deployment_unit: Result<DeploymentUnit>,
        ) -> Result<()> {
            let deployment_unit = deployment_unit.await?;

            assert_eq!(
                Some((
                    "db",
                    &Image::from_str("mariadb").unwrap(),
                    &ContainerType::ApplicationCompanion,
                    &blueprint_service!(
                        "db",
                        "mariadb",
                        env = (
                            "MARIADB_ROOT_PASSWORD" => "example",
                            "MARIADB_USER" => "example-user",
                            "MARIADB_PASSWORD" => "my_cool_secret",
                            "MARIADB_DATABASE" => "example-database"
                        ),
                        files = (
                            "/docker-entrypoint-initdb.d/databases-backup.sql" => "CREATE DATABASE `blog`;CREATE DATABASE `nextcloud`;"
                        )
                    ),
                )),
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "db")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .next(),
            );
            Ok(())
        }

        #[tokio::test]
        async fn use_services_in_application_companions_templating_with_presedence() -> Result<()> {
            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![{
                    let mut nextcloud = nextcloud_config();
                    nextcloud.image = Image::from_str("nextcloud:fpm").unwrap();
                    nextcloud
                }],
                None,
            )
            .with_app_to_replicate_from(AppName::from_str("other").ok())
            .with_static_companions(
                vec![StaticCompanion::app_companion(blueprint_service!(
                    "api-gateway",
                    "nginx",
                    env = (),
                    files = (
                        "/etc/nginx/templates/gateway.template" => r#"
                                {{~#each services}}
                                location /some-prefix/{{name}} {
                                    proxy_pass http://{{name}}:{{port}};
                                }
                                {{/each~}}
                                "#
                    )
                ))]
            )
            .resolve_apps::<anyhow::Error, _>(|app_name| {
                if app_name != AppName::master() {
                    return Ok::<_, anyhow::Error>(Some(App::new(
                        vec![
                            {
                                let mut wordpress = wordpress_config();
                                wordpress.image = Image::from_str("wordpress:fpm").unwrap();
                                wordpress
                            },
                            nextcloud_config(),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(i, blueprint_config)| app_instance::Service {
                            id: i.to_string(),
                            status: app_instance::ServiceStatus::Paused,
                            service_type: ContainerType::Instance,
                            blueprint_config,
                        })
                        .chain(std::iter::once(app_instance::Service {
                            id: String::from("db"),
                            status: app_instance::ServiceStatus::Paused,
                            service_type: ContainerType::ApplicationCompanion,
                            blueprint_config: mariadb_config(),
                        }))
                        .collect::<Vec<_>>(),
                        HashSet::new(),
                        None,
                        None,
                    )));
                }

                Ok::<_, anyhow::Error>(Some(App::new(
                    vec![wordpress_config(), nextcloud_config()]
                        .into_iter()
                        .enumerate()
                        .map(|(i, blueprint_config)| app_instance::Service {
                            id: i.to_string(),
                            status: app_instance::ServiceStatus::Paused,
                            service_type: ContainerType::Instance,
                            blueprint_config,
                        })
                        .chain(std::iter::once(app_instance::Service {
                            id: String::from("db"),
                            status: app_instance::ServiceStatus::Paused,
                            service_type: ContainerType::ApplicationCompanion,
                            blueprint_config: mariadb_config(),
                        }))
                        .collect::<Vec<_>>(),
                    HashSet::new(),
                    None,
                    None,
                )))
            })
            .await?
            .resolve_image_manifests::<anyhow::Error, _>(async |_images| {
                Ok(HashMap::from([
                    (
                        Image::from_str("nextcloud:fpm").unwrap(),
                        ImageInfo::with_exposed_port(3306),
                    ),
                    (
                        Image::from_str("wordpress:fpm").unwrap(),
                        ImageInfo::with_exposed_port(3308),
                    ),
                ]))
            })
            .await?
            .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
            .await?
            .finish()?;

            assert_eq!(
                Some((
                    "api-gateway",
                    &Image::from_str("nginx").unwrap(),
                    &ContainerType::ApplicationCompanion,
                    &blueprint_service!(
                        "api-gateway",
                        "nginx",
                        env = (),
                        files = (
                            "/etc/nginx/templates/gateway.template" => r#"                                location /some-prefix/blog {
                                    proxy_pass http://blog:3308;
                                }
                                location /some-prefix/nextcloud {
                                    proxy_pass http://nextcloud:3306;
                                }
"#
                        )
                    ),
                )),
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "api-gateway")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .next(),
            );
            Ok(())
        }

        #[rstest::rstest]
        #[case::app_companions(vec![
            StaticCompanion::app_companion(mariadb_config()),
            StaticCompanion::app_companion(blueprint_service!("adminer", "adminer:4.8.1"))
                .with_templated_rule(Some(String::from("PathPrefix(`/{{application.name}}/adminer/sub-path`)"))),
        ])]
        #[case::service_companions(vec![
            StaticCompanion::service_companion(mariadb_config()),
            StaticCompanion::service_companion(blueprint_service!("adminer", "adminer:4.8.1"))
                .with_templated_rule(Some(String::from("PathPrefix(`/{{application.name}}/adminer/sub-path`)"))),
        ])]
        fn rule_templating(#[case] static_companions: Vec<StaticCompanion>) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), vec![nextcloud_config()], None)
                    .with_static_companions(static_companions)
                    .finish()?;

            assert_eq!(
                vec![(
                    "adminer",
                    &Image::from_str("adminer:4.8.1").unwrap(),
                    &blueprint_service!("adminer", "adminer:4.8.1"),
                    &TraefikIngressRoute::with_rule(
                        TraefikRouterRule::from_str("PathPrefix(`/master/adminer/sub-path`)")
                            .unwrap()
                    ),
                ),],
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "adminer")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.blueprint_service,
                        &service.ingress_route
                    ))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::app_companions(vec![
            StaticCompanion::app_companion(mariadb_config()),
            StaticCompanion::app_companion(blueprint_service!("adminer", "adminer:4.8.1"))
                .with_templated_rule(Some(String::from("PathPrefix(`/{{application.name}}/adminer/sub-path`)"))),
        ])]
        #[case::service_companions(vec![
            StaticCompanion::service_companion(mariadb_config()),
            StaticCompanion::service_companion(blueprint_service!("adminer", "adminer:4.8.1"))
                .with_templated_rule(Some(String::from("PathPrefix(`/{{application.name}}/adminer/sub-path`)"))),
        ])]
        async fn rule_templating_and_base_route(
            #[case] static_companions: Vec<StaticCompanion>,
        ) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), vec![nextcloud_config()], None)
                    .with_static_companions(static_companions)
                    .resolve_apps::<anyhow::Error, _>(|_app_name| Ok::<_, anyhow::Error>(None))
                    .await?
                    .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                    .await?
                    .resolve_base_route::<anyhow::Error, _>(async || {
                        Ok(Some(TraefikIngressRoute::with_rule(
                            TraefikRouterRule::from_str("Host(`example.com`)").unwrap(),
                        )))
                    })
                    .await?
                    .finish()?;

            assert_eq!(
                vec![(
                    "adminer",
                    &Image::from_str("adminer:4.8.1").unwrap(),
                    &blueprint_service!("adminer", "adminer:4.8.1"),
                    &TraefikIngressRoute::with_rule(
                        TraefikRouterRule::from_str(
                            "Host(`example.com`) && PathPrefix(`/master/adminer/sub-path`)"
                        )
                        .unwrap()
                    ),
                ),],
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "adminer")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.blueprint_service,
                        &service.ingress_route
                    ))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }

        #[rstest::rstest]
        #[case::app_companions(vec![
            StaticCompanion::app_companion(mariadb_config()),
            StaticCompanion::app_companion(blueprint_service!("adminer", "adminer:4.8.1"))
                .with_templated_middlewares(toml::from_str::<BTreeMap<String, serde_value::Value>>(r#"
                    headers = { 'customRequestHeaders' = { 'X-Forwarded-Prefix' =  '/{{application.name}}/adminer/' } }
                "#).ok()),
        ])]
        #[case::service_companions(vec![
            StaticCompanion::service_companion(mariadb_config()),
            StaticCompanion::service_companion(blueprint_service!("adminer", "adminer:4.8.1"))
                .with_templated_middlewares(toml::from_str::<BTreeMap<String, serde_value::Value>>(r#"
                    headers = { 'customRequestHeaders' = { 'X-Forwarded-Prefix' =  '/{{application.name}}/adminer/' } }
                "#).ok()),
        ])]
        fn middleware_templating(#[case] static_companions: Vec<StaticCompanion>) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), vec![nextcloud_config()], None)
                    .with_static_companions(static_companions)
                    .finish()?;

            assert_eq!(
                vec![(
                    "adminer",
                    &Image::from_str("adminer:4.8.1").unwrap(),
                    &blueprint_service!("adminer", "adminer:4.8.1"),
                    &TraefikIngressRoute::with_defaults_and_additional_middleware(
                        &AppName::master(),
                        "adminer",
                        vec![TraefikMiddleware {
                            name: String::from("master-adminer-custom-middleware-0"),
                            spec: serde_value::to_value(serde_json::json!({
                                "headers": {
                                    "customRequestHeaders": {
                                        "X-Forwarded-Prefix": "/master/adminer/"
                                    }
                                }
                            }))
                            .unwrap()
                        }]
                        .into_iter()
                    ),
                ),],
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "adminer")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.blueprint_service,
                        &service.ingress_route
                    ))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }

        #[rstest::rstest]
        #[case::app_companions(vec![
            StaticCompanion::app_companion(mariadb_config()),
            StaticCompanion::app_companion(blueprint_service!("adminer", "adminer:4.8.1"))
                .with_templated_rule(Some(String::from("PathPrefix(`/{{application.name}}/adminer/sub-path`)")))
                .with_templated_middlewares(toml::from_str::<BTreeMap<String, serde_value::Value>>(r#"
                    headers = { 'customRequestHeaders' = { 'X-Forwarded-Prefix' =  '/{{application.name}}/adminer/sub-path' } }
                    stripPrefix = { 'prefixes' = [ '/{{application.name}}/adminer/sub-path' ] }
                "#).ok()),
        ])]
        #[case::service_companions(vec![
            StaticCompanion::service_companion(mariadb_config()),
            StaticCompanion::service_companion(blueprint_service!("adminer", "adminer:4.8.1"))
                .with_templated_rule(Some(String::from("PathPrefix(`/{{application.name}}/adminer/sub-path`)")))
                .with_templated_middlewares(toml::from_str::<BTreeMap<String, serde_value::Value>>(r#"
                    headers = { 'customRequestHeaders' = { 'X-Forwarded-Prefix' =  '/{{application.name}}/adminer/sub-path' } }
                    stripPrefix = { 'prefixes' = [ '/{{application.name}}/adminer/sub-path' ] }
                "#).ok()),
        ])]
        fn rule_and_middleware_templating(
            #[case] static_companions: Vec<StaticCompanion>,
        ) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), vec![nextcloud_config()], None)
                    .with_static_companions(static_companions)
                    .finish()?;

            assert_eq!(
                vec![(
                    "adminer",
                    &Image::from_str("adminer:4.8.1").unwrap(),
                    &blueprint_service!("adminer", "adminer:4.8.1"),
                    &TraefikRouterRule::from_str("PathPrefix(`/master/adminer/sub-path`)").unwrap(),
                    &vec![
                        TraefikMiddleware {
                            name: String::from("master-adminer-custom-middleware-0"),
                            spec: serde_value::to_value(serde_json::json!({
                                "headers": {
                                    "customRequestHeaders": {
                                        "X-Forwarded-Prefix": "/master/adminer/sub-path"
                                    }
                                }
                            }))
                            .unwrap()
                        },
                        TraefikMiddleware {
                            name: String::from("master-adminer-custom-middleware-1"),
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
                ),],
                deployment_unit
                    .services
                    .iter()
                    .filter(|service| service.blueprint_service.service_name == "adminer")
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.blueprint_service,
                        service.ingress_route.routes()[0].rule(),
                        service.ingress_route.routes()[0].middlewares()
                    ))
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }

        #[test]
        fn x() -> Result<()> {
            let mut wordpress_config = wordpress_config();
            wordpress_config.add_env(EnvironmentVariable::with_templating(
                String::from("WORDPRESS_CONFIG_EXTRA"),
                SecUtf8::from_str("define('UNKNOWN_DATA','{{userDefined.a}}');").unwrap(),
            ));

            let deployment_unit = AppDeploymentBuilder::init(
                AppName::master(),
                vec![wordpress_config, mariadb_config()],
                Some(unsafe {
                    UserDefinedParameters::without_validation(serde_json::json!({"a": 1}))
                }),
            )
            .finish()?;

            assert_eq!(
                vec![
                    (
                        "blog",
                        &Image::from_str("wordpress").unwrap(),
                        &ContainerType::Instance,
                        &{
                            let mut expected_wordpress_config = blueprint_service!(
                                "blog",
                                "wordpress",
                                env = (
                                    "WORDPRESS_DB_HOST" => "db",
                                    "WORDPRESS_DB_USER" => "example-user",
                                    "WORDPRESS_DB_PASSWORD" => "my_cool_secret",
                                    "WORDPRESS_DB_NAME" => "example-database"
                                )
                            );
                            expected_wordpress_config.add_env(
                                EnvironmentVariable::with_templating(
                                    String::from("WORDPRESS_CONFIG_EXTRA"),
                                    SecUtf8::from_str("define('UNKNOWN_DATA','1');").unwrap(),
                                ),
                            );
                            expected_wordpress_config
                                .apply_template(&TemplateData {
                                    ..Default::default()
                                })
                                .unwrap()
                        },
                    ),
                    (
                        "db",
                        &Image::from_str("mariadb").unwrap(),
                        &ContainerType::Instance,
                        &blueprint_service!(
                            "db",
                            "mariadb",
                            env = (
                                "MARIADB_ROOT_PASSWORD" => "example",
                                "MARIADB_USER" => "example-user",
                                "MARIADB_PASSWORD" => "my_cool_secret",
                                "MARIADB_DATABASE" => "example-database"
                            )
                        ),
                    ),
                ],
                deployment_unit
                    .services
                    .iter()
                    .map(|service| (
                        service.blueprint_service.service_name.as_str(),
                        &service.blueprint_service.image,
                        &service.service_type,
                        &service.blueprint_service,
                    ))
                    .collect::<Vec<_>>(),
            );

            Ok(())
        }
    }

    mod user_defined_parameters {
        use super::*;
        use pretty_assertions::assert_eq;

        #[tokio::test]
        #[rstest::rstest]
        #[case::pass_from_user_request(
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-string": "data",
                    "test-number": 123,
                    "test-array": [1, 2, 3]
                }))
            }),
            None,
            None,
        )]
        #[case::copy_from_running_app(
            None,
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-string": "data",
                    "test-number": 123,
                    "test-array": [1, 2, 3]
                }))
            }),
            None,
        )]
        #[case::copy_from_app_to_replicate_from(
            None,
            None,
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-string": "data",
                    "test-number": 123,
                    "test-array": [1, 2, 3]
                }))
            }),
        )]
        #[case::user_takes_precedence_over_running(
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-string": "data",
                    "test-number": 123,
                    "test-array": [3]
                }))
            }),
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-number": 456,
                    "test-array": [1, 2]
                }))
            }),
            None,
        )]
        #[case::user_takes_precedence_over_app_to_replicate(
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-string": "data",
                    "test-number": 123,
                    "test-array": [3]
                }))
            }),
            None,
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-number": 456,
                    "test-array": [1, 2]
                }))
            }),
        )]
        #[case::replicate_app_takes_precesdence_over_running(
            None,
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-string": "data",
                    "test-number": 456,
                    "test-array": [1, 2, 3]
                }))
            }),
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-number": 123,
                }))
            }),
        )]
        #[case::merge_all_sources(
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-string": "data",
                }))
            }),
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-number": 123,
                }))
            }),
            Some(unsafe {
                UserDefinedParameters::without_validation(serde_json::json!({
                    "test-array": [1, 2, 3]
                }))
            }),
        )]
        async fn pass_user_defined_parameters(
            #[case] udp: Option<UserDefinedParameters>,
            #[case] udp_running_app: Option<UserDefinedParameters>,
            #[case] udp_app_to_replicate: Option<UserDefinedParameters>,
        ) -> Result<()> {
            let deployment_unit =
                AppDeploymentBuilder::init(AppName::master(), wordpress_configs(), udp)
                    .with_app_to_replicate_from(AppName::from_str("other").ok())
                    .with_static_companions(std::iter::empty())
                    .resolve_apps::<anyhow::Error, _>(move |app_name: AppName| {
                        Ok::<_, anyhow::Error>(Some(App::new(
                            wordpress_configs()
                                .into_iter()
                                .enumerate()
                                .map(|(i, config)| app_instance::Service {
                                    id: i.to_string(),
                                    status: app_instance::ServiceStatus::Paused,
                                    service_type: ContainerType::Instance,
                                    blueprint_config: config,
                                })
                                .collect::<Vec<_>>(),
                            HashSet::new(),
                            match app_name.as_str() {
                                "master" => udp_running_app.clone(),
                                "other" => udp_app_to_replicate.clone(),
                                _ => None,
                            },
                            None,
                        )))
                    })
                    .await?
                    .resolve_image_manifests::<anyhow::Error, _>(async |_images| Ok(HashMap::new()))
                    .await?
                    .resolve_base_route::<anyhow::Error, _>(async || Ok(None))
                    .await?
                    .finish()?;

            assert_eq!(
                Some(unsafe {
                    UserDefinedParameters::without_validation(serde_json::json!({
                        "test-string": "data",
                        "test-number": 123,
                        "test-array": [1, 2, 3]
                    }))
                }),
                deployment_unit.user_defined_parameters
            );
            Ok(())
        }
    }
}
