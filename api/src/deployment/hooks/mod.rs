use self::deployment_hooks::DeploymentHooks;
use self::persistence_hooks::PersistenceHooks;
use super::deployment_unit::{DeployableService, ServicePath};
use crate::apps::AppsServiceError;
use crate::{config::Config, models::AppName};
pub mod deployment_hooks;
pub mod persistence_hooks;

pub struct Hooks<'a> {
    hook_config: &'a Config,
}

impl<'a> Hooks<'a> {
    pub fn new(hook_config: &'a Config) -> Self {
        Hooks { hook_config }
    }

    pub async fn apply_deployment_hook(
        &self,
        app_name: &AppName,
        services: Vec<DeployableService>,
    ) -> Result<Vec<DeployableService>, AppsServiceError> {
        match self.hook_config.hook("deployment") {
            None => Ok(services),
            Some(hook_path) => {
                DeploymentHooks::parse_and_run_hook(app_name, services, hook_path).await
            }
        }
    }

    pub async fn apply_persistence_hooks(
        &self,
        app_name: &AppName,
        services: &Vec<DeployableService>,
    ) -> Result<Vec<Vec<ServicePath>>, AppsServiceError> {
        match self.hook_config.hook("persistence") {
            None => Ok(Hooks::default_persistence_structure(services)),
            Some(hook_path) => {
                PersistenceHooks::parse_and_run_hook(app_name, services, hook_path).await
            }
        }
    }

    pub fn default_persistence_structure(services: &[DeployableService]) -> Vec<Vec<ServicePath>> {
        let mut default_persistence = Vec::new();
        services.iter().for_each(|s| {
            s.declared_volumes().iter().for_each(|v| {
                default_persistence.push(vec![ServicePath::new(
                    s.service_name().to_string(),
                    v.to_string(),
                )]);
            })
        });
        default_persistence
    }
}
