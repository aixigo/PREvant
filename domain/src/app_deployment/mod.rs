//! Based on [`app_blueprints`](`crate::app_blueprints`) this module provides the composition APIs
//! to build a application deployment that will be translated by an actual infrastructure, i.e. at
//! the moment Docker or Kubernetes, into the API call to deploy it.

use crate::{
    AppName, Owner, RawInfrastructureElement,
    app_blueprints::{ServiceConfig, UserDefinedParameters},
    app_instance::{App, ContainerType},
    traefik::TraefikIngressRoute,
};
pub use builder::{
    AppDeploymentBuilder, ApplicationCompanion, BootstrapCompanions,
    BootstrapCompanionsWithRawElementsContext, BootstrappedCompanions,
    BuildDeploymentUintBuildError, Initialized, MergeRawElementsContext, ResolveApps,
    ResolveAppsError, ServiceCompanion, StaticCompanion, StaticCompanionDeploymentStrategy,
    StaticCompanionStorageStrategy, WithResolvedImages, WithStaticCompanions,
};
use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
};

mod builder;

#[derive(Debug, PartialEq, Clone)]
pub struct DeploymentUnit {
    pub app_name: AppName,
    /// The services that have to be deployed on the infrastructure
    pub services: Vec<DeployableService>,
    /// These are infrastructure specific payloads, see [`RawInfrastructureElement`], that originate
    /// from the infrastructure-specific bootstrapping of companions. These elements are treated as
    /// opaque elements by the domain layer.
    pub bootstrapped_companion_elements: Vec<RawInfrastructureElement>,
    /// The Traefik route under which the unit shall be accessible.
    pub route: TraefikIngressRoute,
    pub user_defined_parameters: Option<UserDefinedParameters>,
    pub owners: HashSet<Owner>,
    /// The services running at the time when updating an existing application.
    pub running_application: Option<App>,
    phantom_data: PhantomData<()>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeploymentStrategy {
    Always,
    OnImageUpdate(String),
    Never,
}

/// Describes the state that a running OCI container container must have when a [`DeploymentUnit`]
/// is deployed to an infrastructure.
#[derive(Clone, Debug, PartialEq)]
pub struct DeployableService {
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
    /// These are infrastructure specific payloads, see [`RawInfrastructureElement`], that originate
    /// from the infrastructure-specific bootstrapping of this service. These elements are treated as
    /// opaque elements by the domain layer but indicate to the infrastructure that how this service
    /// was bootstrapped.
    pub bootstrapped_companion_elements: Vec<RawInfrastructureElement>,
    phantom_data: PhantomData<()>,
}
