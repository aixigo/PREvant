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

use chrono::{DateTime, FixedOffset};
pub use docker::Docker;
use domain::{
    AppName,
    app_blueprints::{DesiredServiceStatus, Environment},
    app_deployment::{
        BootstrapCompanionsContext, BootstrappedCompanions, MergeBootstrappedElementsContext,
    },
    app_instance::{App, Service},
    templating::TemplateData,
    traefik::TraefikIngressRoute,
};
#[cfg(test)]
pub use dummy_infrastructure::Dummy;
use futures::stream::BoxStream;
pub use infrastructure::{HttpForwarder, Infrastructure};
pub use kubernetes::Kubernetes;
use regex::Regex;
use serde_json::{Value, map::Map};
use std::collections::HashMap;

mod docker;
#[cfg(test)]
mod dummy_infrastructure;
mod infrastructure;
mod kubernetes;

static APP_NAME_LABEL: &str = "com.aixigo.preview.servant.app-name";
static SERVICE_NAME_LABEL: &str = "com.aixigo.preview.servant.service-name";
static CONTAINER_TYPE_LABEL: &str = "com.aixigo.preview.servant.container-type";
static REPLICATED_ENV_LABEL: &str = "com.aixigo.preview.servant.replicated-env";
static IMAGE_LABEL: &str = "com.aixigo.preview.servant.image";
static STORAGE_TYPE_LABEL: &str = "com.aixigo.preview.servant.storage-type";
static USER_DEFINED_PARAMETERS_LABEL: &str = "com.aixigo.preview.servant.user-defined";
static OWNERS_LABEL: &str = "com.aixigo.preview.servant.owners";

/// This function converts the environment variables and adds all variables, that
/// must be replicated, into a JSON object. This function should be used by implementations
/// to serialize the environment variable so that it can be deserialized when service configurations
/// will be cloned from a running service.
fn replicated_environment_variable_to_json(env: &Environment) -> Option<Value> {
    let replicated_env = env
        .iter()
        .filter(|ev| ev.replicate())
        .map(|ev| {
            (
                ev.key(),
                serde_json::json!({
                    "value": ev.original().value().unsecure(),
                    "templated": ev.templated(),
                    "replicate": true
                }),
            )
        })
        .fold(Map::<String, Value>::new(), |mut acc, (key, value)| {
            acc.insert(key.clone(), value);
            acc
        });

    if !replicated_env.is_empty() {
        Some(Value::Object(replicated_env))
    } else {
        None
    }
}

/// This enum is a proxy for any actual infrastructure implementation.
#[derive(Clone)]
pub enum InfrastructureProxy {
    Docker(Docker),
    Kubernetes(Kubernetes),
    #[cfg(test)]
    Dummy(Dummy),
}

macro_rules! dispatch {
    ($self:ident, $method:ident $(, $args:expr)*) => {
        match $self {
            Self::Docker(d) => d.$method($($args),*),
            Self::Kubernetes(k) => k.$method($($args),*),
            #[cfg(test)]
            Self::Dummy(x) => x.$method($($args),*),
        }
    };
}

#[derive(Clone)]
pub enum BootstrappedElementProxy {
    Kubernetes(<Kubernetes as Infrastructure>::BootstrappedElement),
    Unit,
}

impl From<()> for BootstrappedElementProxy {
    fn from(_value: ()) -> Self {
        Self::Unit
    }
}

impl From<<Kubernetes as Infrastructure>::BootstrappedElement> for BootstrappedElementProxy {
    fn from(value: <Kubernetes as Infrastructure>::BootstrappedElement) -> Self {
        Self::Kubernetes(value)
    }
}

impl Into<<Kubernetes as Infrastructure>::BootstrappedElement> for BootstrappedElementProxy {
    fn into(self) -> <Kubernetes as Infrastructure>::BootstrappedElement {
        match self {
            BootstrappedElementProxy::Kubernetes(e) => e,
            other => panic!(
                "This should never happen: {} must not be converted to {}",
                std::any::type_name::<()>(),
                std::any::type_name_of_val(&other)
            ),
        }
    }
}

impl Into<()> for BootstrappedElementProxy {
    fn into(self) -> () {
        match self {
            BootstrappedElementProxy::Unit => (),
            other => panic!(
                "This should never happen: {} must not be converted to {}",
                std::any::type_name::<()>(),
                std::any::type_name_of_val(&other)
            ),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum BackupPayloadProxy {
    Kubernetes(<Kubernetes as Infrastructure>::BackupPayload),
    // must be placed last, otherwise it will always deserialized
    Unsupported,
}

#[async_trait::async_trait]
impl Infrastructure for InfrastructureProxy {
    type BootstrappedElement = BootstrappedElementProxy;
    type BackupPayload = BackupPayloadProxy;

    async fn fetch_apps(&self) -> anyhow::Result<HashMap<AppName, App>> {
        dispatch!(self, fetch_apps).await
    }

    async fn fetch_traefik_router_names(
        &self,
        app_names: Vec<AppName>,
    ) -> anyhow::Result<HashMap<AppName, Vec<Regex>>> {
        dispatch!(self, fetch_traefik_router_names, app_names).await
    }

    async fn fetch_app(&self, app_name: &AppName) -> anyhow::Result<Option<App>> {
        dispatch!(self, fetch_app, app_name).await
    }

    async fn fetch_app_as_backup_based_infrastructure_payload(
        &self,
        app_name: &AppName,
    ) -> anyhow::Result<Option<Self::BackupPayload>> {
        match self {
            Self::Kubernetes(kubernetes) => Ok(kubernetes
                .fetch_app_as_backup_based_infrastructure_payload(app_name)
                .await?
                .map(BackupPayloadProxy::Kubernetes)),
            _ => {
                anyhow::bail!("Mismatched backend and backup payload")
            }
        }
    }

    async fn deploy_services(
        &self,
        deployment_unit: domain::app_deployment::DeploymentUnit<Self::BootstrappedElement>,
        container_config: &crate::config::ContainerConfig,
    ) -> anyhow::Result<App> {
        match self {
            Self::Docker(docker) => {
                docker
                    .deploy_services(
                        domain::app_deployment::DeploymentUnit::from_bootstrapped(deployment_unit),
                        container_config,
                    )
                    .await
            }
            Self::Kubernetes(kubernetes) => {
                kubernetes
                    .deploy_services(
                        domain::app_deployment::DeploymentUnit::from_bootstrapped(deployment_unit),
                        container_config,
                    )
                    .await
            }
            #[cfg(test)]
            Self::Dummy(dummy) => {
                dummy
                    .deploy_services(
                        domain::app_deployment::DeploymentUnit::from_bootstrapped(deployment_unit),
                        container_config,
                    )
                    .await
            }
        }
    }

    async fn stop_services(&self, app_name: &AppName) -> anyhow::Result<Option<App>> {
        dispatch!(self, stop_services, app_name).await
    }

    async fn restore_infrastructure_objects_partially(
        &self,
        app_name: &AppName,
        payload: Self::BackupPayload,
    ) -> anyhow::Result<Option<App>> {
        match (self, payload) {
            (Self::Kubernetes(kubernetes), BackupPayloadProxy::Kubernetes(payload)) => {
                kubernetes
                    .restore_infrastructure_objects_partially(app_name, payload)
                    .await
            }
            _ => anyhow::bail!("Mismatched backend and backup payload"),
        }
    }

    async fn delete_infrastructure_objects_partially(
        &self,
        app_name: &AppName,
        payload: Self::BackupPayload,
    ) -> anyhow::Result<()> {
        match (self, payload) {
            (Self::Kubernetes(kubernetes), BackupPayloadProxy::Kubernetes(payload)) => {
                kubernetes
                    .delete_infrastructure_objects_partially(app_name, payload)
                    .await
            }
            _ => anyhow::bail!("Mismatched backend and backup payload"),
        }
    }

    async fn get_logs<'a>(
        &'a self,
        app_name: &'a AppName,
        service_name: &'a str,
        from: &'a Option<DateTime<FixedOffset>>,
        limit: &'a Option<usize>,
        follow: bool,
    ) -> BoxStream<'a, anyhow::Result<(DateTime<FixedOffset>, String)>> {
        dispatch!(self, get_logs, app_name, service_name, from, limit, follow).await
    }

    async fn change_status(
        &self,
        app_name: &AppName,
        service_name: &str,
        status: DesiredServiceStatus,
    ) -> anyhow::Result<Option<Service>> {
        dispatch!(self, change_status, app_name, service_name, status).await
    }

    async fn base_traefik_ingress_route(&self) -> anyhow::Result<Option<TraefikIngressRoute>> {
        dispatch!(self, base_traefik_ingress_route).await
    }

    async fn resolve_infrastructure_template_data(
        &self,
        app_name: &AppName,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        dispatch!(self, resolve_infrastructure_template_data, app_name).await
    }

    async fn bootstrap_companions_with_raw_elements(
        &self,
        context: BootstrapCompanionsContext<'_>,
        template_data: &TemplateData,
    ) -> anyhow::Result<BootstrappedCompanions<Self::BootstrappedElement>> {
        match self {
            Self::Docker(docker) => docker
                .bootstrap_companions_with_raw_elements(context, template_data)
                .await
                .map(BootstrappedCompanions::from_bootstrapped),
            Self::Kubernetes(kubernetes) => kubernetes
                .bootstrap_companions_with_raw_elements(context, template_data)
                .await
                .map(BootstrappedCompanions::from_bootstrapped),
            #[cfg(test)]
            Self::Dummy(dummy) => dummy
                .bootstrap_companions_with_raw_elements(context, template_data)
                .await
                .map(BootstrappedCompanions::from_bootstrapped),
        }
    }

    fn update_raw_elements_after_merged_blueprint_config(
        &self,
        context: MergeBootstrappedElementsContext<'_>,
        raw_elements: Vec<Self::BootstrappedElement>,
    ) -> anyhow::Result<Vec<Self::BootstrappedElement>> {
        Ok(match self {
            Self::Docker(_) => Vec::new(),
            Self::Kubernetes(kubernetes) => kubernetes
                .update_raw_elements_after_merged_blueprint_config(
                    context,
                    raw_elements.into_iter().map(Into::into).collect(),
                )?
                .into_iter()
                .map(BootstrappedElementProxy::Kubernetes)
                .collect::<Vec<_>>(),
            #[cfg(test)]
            Self::Dummy(_) => Vec::new(),
        })
    }

    async fn http_forwarder(&self) -> anyhow::Result<Box<dyn HttpForwarder>> {
        dispatch!(self, http_forwarder).await
    }
}

impl From<Docker> for InfrastructureProxy {
    fn from(docker: Docker) -> Self {
        Self::Docker(docker)
    }
}

impl From<Kubernetes> for InfrastructureProxy {
    fn from(kubernetes: Kubernetes) -> Self {
        Self::Kubernetes(kubernetes)
    }
}

#[cfg(test)]
impl From<Dummy> for InfrastructureProxy {
    fn from(dummy: Dummy) -> Self {
        Self::Dummy(dummy)
    }
}

#[cfg(test)]
mod tests {

    mod parse_backup_payload_proxy {
        use super::super::*;

        #[test]
        fn k8s() -> anyhow::Result<()> {
            let deployment = r#"
- apiVersion: apps/v1
  kind: Deployment
  metadata:
    name: simple-deployment
  spec:
    replicas: 3
    selector:
      matchLabels:
        app: simple-app
    template:
      metadata:
        labels:
          app: simple-app
      spec:
        containers:
        - name: simple-container
          image: nginx:latest
          ports:
          - containerPort: 80
"#;

            let deployment = noyalib::from_str::<BackupPayloadProxy>(deployment)?;

            assert!(matches!(deployment, BackupPayloadProxy::Kubernetes(e) if !e.is_empty()));
            Ok(())
        }

        #[test]
        fn unknown_variant() {
            let deployment = r#"{}"#;

            let result = serde_json::from_str::<BackupPayloadProxy>(deployment);

            assert!(result.is_err());
        }

        #[test]
        fn unsupported() -> anyhow::Result<()> {
            let deployment = r#""#;

            let deployment = noyalib::from_str::<BackupPayloadProxy>(deployment)?;

            assert!(matches!(deployment, BackupPayloadProxy::Unsupported));
            Ok(())
        }
    }
}
