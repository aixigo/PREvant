//! Based on [`app_blueprints`](`crate::app_blueprints`) this module provides the composition APIs
//! to build a application deployment that will be translated by an actual infrastructure, i.e. at
//! the moment Docker or Kubernetes, into the API call to deploy it.

use crate::{
    AppName, Owner,
    app_blueprints::{ServiceConfig, UserDefinedParameters},
    app_instance::ContainerType,
    traefik::TraefikIngressRoute,
};
pub use builder::{
    AppDeploymentBuilder, ApplicationCompanion, BootstrapCompanions, BootstrapCompanionsContext,
    BootstrappedCompanion, BootstrappedCompanions, BuildDeploymentUintBuildError, Initialized,
    MergeBootstrappedElementsContext, ResolveApps, ResolveAppsError, ServiceCompanion,
    StaticCompanion, StaticCompanionDeploymentStrategy, StaticCompanionStorageStrategy,
    WithBaseRoute, WithInfrastructureTemplateData, WithResolvedApps, WithResolvedImages,
    WithStaticCompanions,
};
use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
};

mod builder;

/// Describes the state that a set of running OCI containers is deployed to an infrastructure.
///
/// Use [`AppDeploymentBuilder`] in order to create such an instance.
#[derive(Debug, PartialEq, Clone)]
pub struct DeploymentUnit<E>
where
    E: Clone,
{
    pub app_name: AppName,
    /// The services that have to be deployed on the infrastructure
    pub services: Vec<DeployableService<E>>,
    /// These are infrastructure specific, opaque payloads, that originate from the
    /// infrastructure-specific bootstrapping of companions. These elements are treated as opaque
    /// elements by the domain layer.
    pub bootstrapped_companion_elements: Vec<E>,
    /// The Traefik route under which the unit shall be accessible.
    pub route: TraefikIngressRoute,
    pub user_defined_parameters: Option<UserDefinedParameters>,
    pub owners: HashSet<Owner>,
    phantom_data: PhantomData<()>,
}

impl<E> DeploymentUnit<E>
where
    E: Clone,
{
    pub fn from_unbootstrapped(other: DeploymentUnit<()>) -> Self {
        assert!(other.bootstrapped_companion_elements.is_empty());
        Self {
            app_name: other.app_name,
            services: other
                .services
                .into_iter()
                .map(DeployableService::from_unbootstrapped)
                .collect::<Vec<_>>(),
            bootstrapped_companion_elements: Vec::new(),
            route: other.route,
            user_defined_parameters: other.user_defined_parameters,
            owners: other.owners,
            phantom_data: PhantomData,
        }
    }

    pub fn from_bootstrapped<A>(other: DeploymentUnit<A>) -> Self
    where
        A: Clone,
        A: Into<E>,
    {
        Self {
            app_name: other.app_name,
            services: other
                .services
                .into_iter()
                .map(DeployableService::from_bootstrapped)
                .collect::<Vec<_>>(),
            bootstrapped_companion_elements: other
                .bootstrapped_companion_elements
                .into_iter()
                .map(A::into)
                .collect::<Vec<_>>(),
            route: other.route,
            user_defined_parameters: other.user_defined_parameters,
            owners: other.owners,
            phantom_data: PhantomData,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeploymentStrategy {
    Always,
    OnImageUpdate(String),
    Never,
}

/// Describes the state that a running OCI container must have when a [`DeploymentUnit`] is deployed
/// to an infrastructure.
#[derive(Clone, Debug, PartialEq)]
pub struct DeployableService<E>
where
    E: Clone,
{
    pub blueprint_service: ServiceConfig,
    /// Specifies for the infrastructure how this service shall be labeled.
    pub service_type: ContainerType,
    pub strategy: DeploymentStrategy,
    /// The specific Traefik route how the service is exposed. See [router
    /// documentation](https://doc.traefik.io/traefik/reference/routing-configuration/http/routing/router/)
    /// for more information.
    pub ingress_route: TraefikIngressRoute,
    /// Depending on the [`StaticCompanionStorageStrategy`] declared OCI image volumes might be
    /// listed here so that the infrastructure must create the volumes so hold the data.
    pub declared_volumes: Vec<String>,
    /// A set of labels that should be attached to the running OCI container.
    pub labels: HashMap<String, String>,
    /// The port that is exposed by the service.
    pub port: u16,
    /// These are infrastructure specific, opaque payloads, that originate from the
    /// infrastructure-specific bootstrapping of this service. These elements are treated as opaque
    /// elements by the domain layer but indicate to the infrastructure that how this service was
    /// bootstrapped.
    pub bootstrapped_companion_elements: Vec<E>,
    phantom_data: PhantomData<()>,
}

impl<E> DeployableService<E>
where
    E: Clone,
{
    pub fn from_unbootstrapped(other: DeployableService<()>) -> Self {
        assert!(other.bootstrapped_companion_elements.is_empty());

        Self {
            blueprint_service: other.blueprint_service,
            service_type: other.service_type,
            strategy: other.strategy,
            ingress_route: other.ingress_route,
            declared_volumes: other.declared_volumes,
            labels: other.labels,
            port: other.port,
            bootstrapped_companion_elements: Vec::new(),
            phantom_data: PhantomData,
        }
    }

    pub fn from_bootstrapped<A>(other: DeployableService<A>) -> Self
    where
        A: Clone,
        A: Into<E>,
    {
        Self {
            blueprint_service: other.blueprint_service,
            service_type: other.service_type,
            strategy: other.strategy,
            ingress_route: other.ingress_route,
            declared_volumes: other.declared_volumes,
            labels: other.labels,
            port: other.port,
            bootstrapped_companion_elements: other
                .bootstrapped_companion_elements
                .into_iter()
                .map(A::into)
                .collect::<Vec<_>>(),
            phantom_data: PhantomData,
        }
    }
}
