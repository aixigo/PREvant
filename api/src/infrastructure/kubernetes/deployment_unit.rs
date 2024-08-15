use super::{
    infrastructure::KubernetesInfrastructureError,
    payloads::{
        convert_k8s_ingress_to_traefik_ingress, IngressRoute as TraefikIngressRoute,
        Middleware as TraefikMiddleware,
    },
};
use crate::{
    config::BootstrappingContainer,
    deployment::DeploymentUnit,
    infrastructure::{APP_NAME_LABEL, CONTAINER_TYPE_LABEL, SERVICE_NAME_LABEL},
    models::{AppName, ContainerType, Image},
};
use anyhow::Result;
use futures::{AsyncBufReadExt, AsyncReadExt, StreamExt, TryStreamExt};
use handlebars::RenderError;
use k8s_openapi::{
    api::{
        apps::v1::{Deployment, StatefulSet},
        batch::v1::Job,
        core::v1::{
            ConfigMap, Container, LocalObjectReference, PersistentVolumeClaim, Pod, PodSpec,
            Secret, Service, ServiceAccount,
        },
        networking::v1::Ingress,
        rbac::v1::{Role, RoleBinding},
    },
    apimachinery::pkg::apis::meta::v1::LabelSelector,
    DeepMerge, Metadata, Resource,
};
use kube::{
    api::{LogParams, Patch, PatchParams, PostParams, WatchParams},
    core::{DynamicObject, ObjectMeta, WatchEvent},
    Api, Client, ResourceExt,
};
use serde::Deserialize;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashSet},
    str::FromStr,
};

#[derive(Default)]
pub(super) struct K8sDeploymentUnit {
    roles: Vec<Role>,
    role_bindings: Vec<RoleBinding>,
    stateful_sets: Vec<StatefulSet>,
    config_maps: Vec<ConfigMap>,
    secrets: Vec<Secret>,
    pvcs: Vec<PersistentVolumeClaim>,
    services: Vec<Service>,
    pods: Vec<Pod>,
    deployments: Vec<Deployment>,
    jobs: Vec<Job>,
    service_accounts: Vec<ServiceAccount>,
    traefik_ingresses: Vec<TraefikIngressRoute>,
    traefik_middlewares: Vec<TraefikMiddleware>,
}

impl K8sDeploymentUnit {
    async fn start_bootstrapping_pods(
        app_name: &AppName,
        client: Client,
        bootstrapping_containers: &[BootstrappingContainer],
        image_pull_secret: Option<Secret>,
    ) -> Result<(String, Vec<impl AsyncBufReadExt>)> {
        let image_pull_secrets = match image_pull_secret {
            Some(image_pull_secret) => {
                let image_pull_secrets = vec![LocalObjectReference {
                    name: Some(image_pull_secret.metadata.name.clone().unwrap_or_default()),
                }];
                create_or_patch(client.clone(), app_name, image_pull_secret).await?;
                Some(image_pull_secrets)
            }
            None => None,
        };

        let containers = bootstrapping_containers
            .iter()
            .enumerate()
            .map(|(i, bc)| {
                Ok(Container {
                    name: format!("bootstrap-{i}"),
                    image: Some(bc.image().to_string()),
                    image_pull_policy: Some(String::from("Always")),
                    args: Some(bc.args().to_vec()),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, RenderError>>()?;

        let pod_name = format!(
            "{}-bootstrap-{}",
            app_name.to_rfc1123_namespace_id(),
            uuid::Uuid::new_v4()
        );

        let pod = Pod {
            metadata: ObjectMeta {
                name: Some(pod_name.clone()),
                labels: Some(BTreeMap::from([(
                    APP_NAME_LABEL.to_string(),
                    app_name.to_string(),
                )])),
                ..Default::default()
            },
            spec: Some(PodSpec {
                containers,
                image_pull_secrets,
                restart_policy: Some(String::from("Never")),
                ..Default::default()
            }),
            ..Default::default()
        };
        create_or_patch(client.clone(), app_name, pod).await?;

        let api: Api<Pod> = Api::namespaced(client, &app_name.to_rfc1123_namespace_id());

        // Wait for a bookmark event to be sure that the log is ready to be consumed
        let wp = WatchParams::default()
            .fields(&format!("metadata.name={pod_name}"))
            .timeout(10);
        let mut stream = api.watch(&wp, "0").await?.boxed();
        while let Some(status) = stream.try_next().await? {
            trace!("Saw watch event for bootstrapping pod {pod_name} in {app_name}: {status:?}");

            if let WatchEvent::Bookmark(_bookmark) = status {
                debug!("Boot strapping pod {pod_name} for {app_name} ready.");
                break;
            }
        }

        loop {
            let pod = api.get_status(&pod_name).await?;

            if let Some(phase) = pod.status.and_then(|status| status.phase) {
                match phase.as_str() {
                    "Running" | "Succeeded" => {
                        break;
                    }
                    "Failed" | "Unknown" => {
                        return Err(KubernetesInfrastructureError::BootstrapContainerFailed {
                            pod_name,
                            app_name: app_name.clone(),
                        }
                        .into());
                    }
                    phase => {
                        trace!("Boot strapping pod {pod_name} for {app_name} still not in running phase. Currently in {phase}.");
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }

        let mut log_streams = Vec::with_capacity(bootstrapping_containers.len());

        for i in 0..bootstrapping_containers.len() {
            log_streams.push(
                api.log_stream(
                    &pod_name,
                    &LogParams {
                        container: Some(format!("bootstrap-{i}")),
                        follow: true,
                        ..Default::default()
                    },
                )
                .await?,
            );
        }

        Ok((pod_name, log_streams))
    }

    pub(super) async fn bootstrap(
        deployment_unit: &DeploymentUnit,
        client: Client,
        bootstrapping_container: &[BootstrappingContainer],
        image_pull_secret: Option<Secret>,
    ) -> Result<Self> {
        if bootstrapping_container.is_empty() {
            return Ok(Default::default());
        }

        let app_name = deployment_unit.app_name();

        let (bootstrapping_pod_name, mut log_streams) = Self::start_bootstrapping_pods(
            app_name,
            client.clone(),
            bootstrapping_container,
            image_pull_secret,
        )
        .await?;

        let result = Self::parse_from_log_streams(deployment_unit, &mut log_streams).await;

        let pod_api: Api<Pod> = Api::namespaced(client, &app_name.to_rfc1123_namespace_id());
        pod_api
            .delete(&bootstrapping_pod_name, &Default::default())
            .await?;

        result
    }

    async fn parse_from_log_streams<L>(
        deployment_unit: &DeploymentUnit,
        log_streams: L,
    ) -> Result<Self>
    where
        L: IntoIterator,
        <L as IntoIterator>::Item: AsyncBufReadExt,
        <L as IntoIterator>::Item: Unpin,
    {
        let app_name = deployment_unit.app_name();
        let mut roles = Vec::new();
        let mut role_bindings = Vec::new();
        let mut stateful_sets = Vec::new();
        let mut config_maps = Vec::new();
        let mut secrets = Vec::new();
        let mut pvcs = Vec::new();
        let mut services = Vec::new();
        let mut pods = Vec::new();
        let mut deployments = Vec::new();
        let mut jobs = Vec::new();
        let mut service_accounts = Vec::new();
        let mut ingresses = Vec::new();

        for mut log_stream in log_streams.into_iter() {
            let mut stdout = String::new();
            log_stream.read_to_string(&mut stdout).await?;

            trace!(
                "Received YAML from bootstrapping container in {app_name}: {}…",
                stdout.lines().next().unwrap_or(&stdout)
            );

            for doc in serde_yaml::Deserializer::from_str(&stdout) {
                match DynamicObject::deserialize(doc) {
                    Ok(mut dy) => {
                        dy.metadata.namespace = Some(app_name.to_rfc1123_namespace_id());
                        dy.labels_mut()
                            .insert(APP_NAME_LABEL.to_string(), app_name.to_string());

                        let api_version = dy
                            .types
                            .as_ref()
                            .map(|t| t.api_version.as_str())
                            .unwrap_or_default();
                        let kind = dy
                            .types
                            .as_ref()
                            .map(|t| t.kind.as_str())
                            .unwrap_or_default();

                        trace!(
                            "Parsed {} ({api_version}, {kind}) for {app_name} as a bootstrap application element.",
                            dy.metadata
                                .name
                                .as_deref()
                                .unwrap_or_default()
                        );

                        match (api_version, kind) {
                            (Role::API_VERSION, Role::KIND) => match dy.clone().try_parse::<Role>()
                            {
                                Ok(role) => {
                                    roles.push(role);
                                }
                                Err(e) => {
                                    error!("Cannot parse {:?} as Role: {e}", dy.metadata.name);
                                }
                            },

                            (RoleBinding::API_VERSION, RoleBinding::KIND) => {
                                match dy.clone().try_parse::<RoleBinding>() {
                                    Ok(role_binding) => {
                                        role_bindings.push(role_binding);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as RoleBinding: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            (StatefulSet::API_VERSION, StatefulSet::KIND) => {
                                match dy.clone().try_parse::<StatefulSet>() {
                                    Ok(stateful_set) => {
                                        stateful_sets.push(stateful_set);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as StatefulSet: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            (ConfigMap::API_VERSION, ConfigMap::KIND) => {
                                match dy.clone().try_parse::<ConfigMap>() {
                                    Ok(config_map) => {
                                        config_maps.push(config_map);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as ConfigMap: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            (Secret::API_VERSION, Secret::KIND) => {
                                if let serde_json::Value::Object(obj) = &mut dy.data {
                                    obj.entry("data").and_modify(|obj| {
                                        if let serde_json::Value::Object(obj) = obj {
                                            for (_k, v) in obj.iter_mut() {
                                                if let serde_json::Value::String(str) = v {
                                                    // replacing new lines here because it is assumed
                                                    // that the data is base64 encoded and thus there
                                                    // must be no new lines
                                                    *v = str.replace('\n', "").into();
                                                }
                                            }
                                        }
                                    });
                                }

                                match dy.clone().try_parse::<Secret>() {
                                    Ok(secret) => {
                                        secrets.push(secret);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as Secret: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            (PersistentVolumeClaim::API_VERSION, PersistentVolumeClaim::KIND) => {
                                match dy.clone().try_parse::<PersistentVolumeClaim>() {
                                    Ok(pvc) => {
                                        pvcs.push(pvc);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as PersistentVolumeClaim: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            (Service::API_VERSION, Service::KIND) => {
                                match dy.clone().try_parse::<Service>() {
                                    Ok(service) => {
                                        services.push(service);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as Service: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            (Deployment::API_VERSION, Deployment::KIND) => {
                                match dy.clone().try_parse::<Deployment>() {
                                    Ok(mut deployment) => {
                                        let service_name = deployment
                                            .labels()
                                            .get("app.kubernetes.io/component")
                                            .cloned()
                                            .unwrap_or_else(|| {
                                                deployment.metadata.name.clone().unwrap_or_default()
                                            });

                                        deployment
                                            .labels_mut()
                                            .insert(SERVICE_NAME_LABEL.to_string(), service_name);
                                        deployment.labels_mut().insert(
                                            CONTAINER_TYPE_LABEL.to_string(),
                                            ContainerType::ApplicationCompanion.to_string(),
                                        );

                                        deployments.push(deployment);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as Deployment: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            (Pod::API_VERSION, Pod::KIND) => match dy.clone().try_parse::<Pod>() {
                                Ok(pod) => {
                                    pods.push(pod);
                                }
                                Err(e) => {
                                    error!("Cannot parse {:?} as Pod: {e}", dy.metadata.name);
                                }
                            },
                            (Job::API_VERSION, Job::KIND) => match dy.clone().try_parse::<Job>() {
                                Ok(job) => {
                                    jobs.push(job);
                                }
                                Err(e) => {
                                    error!("Cannot parse {:?} as Job: {e}", dy.metadata.name);
                                }
                            },
                            (ServiceAccount::API_VERSION, ServiceAccount::KIND) => {
                                match dy.clone().try_parse::<ServiceAccount>() {
                                    Ok(service_account) => {
                                        service_accounts.push(service_account);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as ServiceAccount: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            (Ingress::API_VERSION, Ingress::KIND) => {
                                match dy.clone().try_parse::<Ingress>() {
                                    Ok(ingress) => {
                                        ingresses.push(ingress);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Cannot parse {:?} as Ingress: {e}",
                                            dy.metadata.name
                                        );
                                    }
                                }
                            }
                            _ => {
                                warn!(
                                    "Cannot parse {name} ({api_version}, {kind}) for {app_name} because its kind is unknown",
                                    name=dy.metadata.name.unwrap_or_default()
                                );
                            }
                        }
                    }
                    Err(err) => {
                        warn!("The output of a bootstrap container for {app_name} could not be parsed: {stdout}");
                        return Err(err.into());
                    }
                }
            }
        }

        let mut traefik_ingresses = Vec::new();
        let mut traefik_middlewares = Vec::new();

        for ingress in ingresses {
            let Ok((route, middlewares)) = convert_k8s_ingress_to_traefik_ingress(
                ingress,
                deployment_unit.app_base_route().clone(),
            ) else {
                continue;
            };

            traefik_ingresses.push(route);
            traefik_middlewares.extend(middlewares);
        }

        Ok(Self {
            roles,
            role_bindings,
            stateful_sets,
            config_maps,
            secrets,
            pvcs,
            services,
            pods,
            deployments,
            jobs,
            service_accounts,
            traefik_ingresses,
            traefik_middlewares,
        })
    }

    pub(super) fn merge(
        &mut self,
        secret: Option<Secret>,
        service: Service,
        deployment: Deployment,
        ingress: TraefikIngressRoute,
        middlewares: Vec<TraefikMiddleware>,
    ) {
        let mut deployment = deployment;

        let service_name = deployment
            .metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(SERVICE_NAME_LABEL))
            .expect("There must be label providing the service name");

        let stateful_sets = self
            .stateful_sets
            .iter_mut()
            .filter(|set| Some(service_name) == set.metadata().name.as_ref())
            .filter_map(|set| {
                let spec = set.spec.as_mut()?;
                Some((
                    &mut set.metadata,
                    spec.template.metadata.as_mut(),
                    spec.template.spec.as_mut()?,
                ))
            });
        let deployments = self
            .deployments
            .iter_mut()
            .filter(|set| Some(service_name) == set.metadata().name.as_ref())
            .filter_map(|deployment| {
                let spec = deployment.spec.as_mut()?;
                Some((
                    &mut deployment.metadata,
                    spec.template.metadata.as_mut(),
                    spec.template.spec.as_mut()?,
                ))
            });
        let pods = self
            .pods
            .iter_mut()
            .filter(|pod| Some(service_name) == pod.metadata().name.as_ref())
            .filter_map(|pod| Some((&mut pod.metadata, None, pod.spec.as_mut()?)));

        match stateful_sets.chain(deployments).chain(pods).next() {
            Some((metadata, pod_meta, pod_spec)) => {
                // Clean everything that might interfere with the original definitions of
                // bootstrapped companion before calling merge_from down below.
                deployment.metadata.name = None;

                metadata.merge_from(deployment.metadata);

                let mut deployment_spec = deployment
                    .spec
                    .expect("There should be a deployment spec created for the deployable service");

                deployment_spec.selector = LabelSelector::default();

                let template_to_be_merged = deployment_spec.template;

                if let Some(pod_meta) = pod_meta {
                    pod_meta.merge_from(
                        template_to_be_merged.metadata.expect(
                            "There should be a pod meta created for the deployable service",
                        ),
                    );
                }

                let mut pod_spec_to_be_merged = template_to_be_merged
                    .spec
                    .expect("There should be a pod spec created for the deployable service");
                pod_spec_to_be_merged.containers[0].name = pod_spec.containers[0].name.clone();
                pod_spec_to_be_merged.containers[0].ports = None;

                pod_spec.merge_from(pod_spec_to_be_merged);

                if let Some(secret) = secret {
                    self.secrets.push(secret);
                }
                // Ingress, Service, and Middlewares will be ignored because at this point it can
                // be assumed that these configurations are covered by the Kubernetes objects that
                // were used for bootstrapping the application.
            }
            None => {
                self.secrets.extend(secret);
                self.services.push(service);
                self.deployments.push(deployment);
                self.traefik_ingresses.push(ingress);
                self.traefik_middlewares.extend(middlewares);
            }
        }
    }

    /// This filters bootstrapped [Deployments](Deployment), [Stateful Sets](StatefulSet), or
    /// [Pods](Pod) by the existing [services](Service) in already deployed application to avoid
    /// that deployments of instances overwrite each other
    pub(super) fn filter_by_instances_and_replicas<S>(&mut self, services: S)
    where
        S: Iterator,
        <S as Iterator>::Item: Borrow<crate::models::service::Service>,
    {
        let service_not_to_be_retained = services
            .filter(|s| {
                s.borrow().container_type() == &ContainerType::Instance
                    || s.borrow().container_type() == &ContainerType::Replica
            })
            .map(|s| s.borrow().service_name().clone())
            .collect::<HashSet<_>>();

        self.deployments.retain(|deployment| {
            let Some(service_name) = deployment
                .metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get(SERVICE_NAME_LABEL))
            else {
                return false;
            };

            !service_not_to_be_retained.contains(service_name)
        });
    }

    fn images_of_pod_spec(spec: &PodSpec) -> HashSet<Image> {
        let mut images = HashSet::new();

        if let Some(init_containers) = &spec.init_containers {
            for init_container in init_containers {
                if let Some(image) = init_container
                    .image
                    .as_ref()
                    .and_then(|image| Image::from_str(image).ok())
                {
                    images.insert(image);
                }
            }
        }

        for container in &spec.containers {
            if let Some(image) = container
                .image
                .as_ref()
                .and_then(|image| Image::from_str(image).ok())
            {
                images.insert(image);
            }
        }

        images
    }

    pub(super) fn images(&self) -> HashSet<Image> {
        let mut images = HashSet::new();

        for deployment in &self.deployments {
            let Some(spec) = &deployment.spec else {
                continue;
            };
            let Some(spec) = &spec.template.spec else {
                continue;
            };

            images.extend(Self::images_of_pod_spec(spec));
        }
        for job in &self.jobs {
            let Some(spec) = &job.spec else {
                continue;
            };
            let Some(spec) = &spec.template.spec else {
                continue;
            };

            images.extend(Self::images_of_pod_spec(spec));
        }
        for stateful_set in &self.stateful_sets {
            let Some(spec) = &stateful_set.spec else {
                continue;
            };
            let Some(spec) = &spec.template.spec else {
                continue;
            };

            images.extend(Self::images_of_pod_spec(spec));
        }
        for pod in &self.pods {
            let Some(spec) = &pod.spec else {
                continue;
            };

            images.extend(Self::images_of_pod_spec(spec));
        }

        images
    }

    pub(super) fn apply_image_pull_secret(&mut self, image_pull_secret: Secret) {
        let pull_secret_reference = LocalObjectReference {
            name: Some(image_pull_secret.metadata.name.clone().unwrap_or_default()),
        };
        self.secrets.push(image_pull_secret);

        for deployment in self.deployments.iter_mut() {
            let Some(spec) = &mut deployment.spec else {
                continue;
            };
            let Some(spec) = &mut spec.template.spec else {
                continue;
            };

            spec.image_pull_secrets = Some(vec![pull_secret_reference.clone()]);
        }
        for job in self.jobs.iter_mut() {
            let Some(spec) = &mut job.spec else {
                continue;
            };
            let Some(spec) = &mut spec.template.spec else {
                continue;
            };

            spec.image_pull_secrets = Some(vec![pull_secret_reference.clone()]);
        }
        for stateful_set in self.stateful_sets.iter_mut() {
            let Some(spec) = &mut stateful_set.spec else {
                continue;
            };
            let Some(spec) = &mut spec.template.spec else {
                continue;
            };

            spec.image_pull_secrets = Some(vec![pull_secret_reference.clone()]);
        }
        for pod in self.pods.iter_mut() {
            let Some(spec) = &mut pod.spec else {
                continue;
            };

            spec.image_pull_secrets = Some(vec![pull_secret_reference.clone()]);
        }
    }

    pub(super) async fn deploy(
        self,
        client: Client,
        app_name: &AppName,
    ) -> Result<Vec<Deployment>> {
        let mut deployments = Vec::with_capacity(self.deployments.len());

        for role in self.roles {
            create_or_patch(client.clone(), app_name, role).await?;
        }
        for role_binding in self.role_bindings {
            create_or_patch(client.clone(), app_name, role_binding).await?;
        }
        for config_map in self.config_maps {
            create_or_patch(client.clone(), app_name, config_map).await?;
        }
        for secret in self.secrets {
            create_or_patch(client.clone(), app_name, secret).await?;
        }
        for pvc in self.pvcs {
            create_or_patch(client.clone(), app_name, pvc).await?;
        }
        for service in self.services {
            create_or_patch(client.clone(), app_name, service).await?;
        }
        for service_account in self.service_accounts {
            create_or_patch(client.clone(), app_name, service_account).await?;
        }
        for deployment in self.deployments {
            let deployment = create_or_patch(client.clone(), app_name, deployment).await?;
            deployments.push(deployment);
        }
        for job in self.jobs {
            create_or_patch(client.clone(), app_name, job).await?;
        }
        for stateful_set in self.stateful_sets {
            create_or_patch(client.clone(), app_name, stateful_set).await?;
        }
        for ingress in self.traefik_ingresses {
            create_or_patch(client.clone(), app_name, ingress).await?;
        }
        for middleware in self.traefik_middlewares {
            create_or_patch(client.clone(), app_name, middleware).await?;
        }
        for pod in self.pods {
            create_or_patch(client.clone(), app_name, pod).await?;
        }

        Ok(deployments)
    }
}

async fn create_or_patch<T>(client: Client, app_name: &AppName, payload: T) -> Result<T>
where
    T: serde::Serialize + Clone + std::fmt::Debug + for<'a> serde::Deserialize<'a>,
    T: kube::core::Resource<Scope = kube::core::NamespaceResourceScope>,
    <T as kube::Resource>::DynamicType: std::default::Default,
{
    let api = Api::namespaced(client.clone(), &app_name.to_rfc1123_namespace_id());
    match api.create(&PostParams::default(), &payload).await {
        Ok(result) => Ok(result),
        Err(kube::error::Error::Api(kube::error::ErrorResponse { code, .. })) if code == 409 => {
            let name = payload.meta().name.clone().unwrap_or_default();
            match api
                .patch(&name, &PatchParams::default(), &Patch::Merge(&payload))
                .await
            {
                Ok(result) => Ok(result),
                Err(_e) => {
                    // TODO: how to handle the case? e.g. patching a job may fails
                    Ok(payload)
                }
            }
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{deployment::deployment_unit::DeploymentUnitBuilder, models::ServiceBuilder};
    use k8s_openapi::api::{
        apps::v1::DeploymentSpec,
        core::v1::{ContainerPort, EnvVar, PodTemplateSpec},
    };
    use std::collections::HashMap;

    async fn parse_unit(stdout: &'static str) -> K8sDeploymentUnit {
        let log_streams = vec![stdout.as_bytes()];

        let deployment_unit = DeploymentUnitBuilder::init(AppName::master(), Vec::new())
            .extend_with_config(&Default::default())
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .apply_templating(&None)
            .unwrap()
            .apply_hooks(&Default::default())
            .await
            .unwrap()
            .apply_base_traefik_ingress_route(
                crate::infrastructure::TraefikIngressRoute::with_app_only_defaults(
                    &AppName::master(),
                ),
            )
            .build();

        K8sDeploymentUnit::parse_from_log_streams(&deployment_unit, log_streams)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn parse_unit_from_secret_stdout_where_value_is_base64_encoded() {
        let unit = parse_unit(
            r#"
            apiVersion: v1
            kind: Secret
            metadata:
              name: secret-tls
            type: kubernetes.io/tls
            data:
              # values are base64 encoded, which obscures them but does NOT provide
              # any useful level of confidentiality
              tls.crt: |
                LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCk1JSUNVakNDQWJzQ0FnMytNQTBHQ1NxR1NJYjNE
                UUVCQlFVQU1JR2JNUXN3Q1FZRFZRUUdFd0pLVURFT01Bd0cKQTFVRUNCTUZWRzlyZVc4eEVEQU9C
                Z05WQkFjVEIwTm9kVzh0YTNVeEVUQVBCZ05WQkFvVENFWnlZVzVyTkVSRQpNUmd3RmdZRFZRUUxF
                dzlYWldKRFpYSjBJRk4xY0hCdmNuUXhHREFXQmdOVkJBTVREMFp5WVc1ck5FUkVJRmRsCllpQkRR
                VEVqTUNFR0NTcUdTSWIzRFFFSkFSWVVjM1Z3Y0c5eWRFQm1jbUZ1YXpSa1pDNWpiMjB3SGhjTk1U
                TXcKTVRFeE1EUTFNVE01V2hjTk1UZ3dNVEV3TURRMU1UTTVXakJMTVFzd0NRWURWUVFHREFKS1VE
                RVBNQTBHQTFVRQpDQXdHWEZSdmEzbHZNUkV3RHdZRFZRUUtEQWhHY21GdWF6UkVSREVZTUJZR0Ex
                VUVBd3dQZDNkM0xtVjRZVzF3CmJHVXVZMjl0TUlHYU1BMEdDU3FHU0liM0RRRUJBUVVBQTRHSUFE
                Q0JoQUo5WThFaUhmeHhNL25PbjJTbkkxWHgKRHdPdEJEVDFKRjBReTliMVlKanV2YjdjaTEwZjVN
                Vm1UQllqMUZTVWZNOU1vejJDVVFZdW4yRFljV29IcFA4ZQpqSG1BUFVrNVd5cDJRN1ArMjh1bklI
                QkphVGZlQ09PekZSUFY2MEdTWWUzNmFScG04L3dVVm16eGFLOGtCOWVaCmhPN3F1TjdtSWQxL2pW
                cTNKODhDQXdFQUFUQU5CZ2txaGtpRzl3MEJBUVVGQUFPQmdRQU1meTQzeE15OHh3QTUKVjF2T2NS
                OEtyNWNaSXdtbFhCUU8xeFEzazlxSGtyNFlUY1JxTVQ5WjVKTm1rWHYxK2VSaGcwTi9WMW5NUTRZ
                RgpnWXcxbnlESnBnOTduZUV4VzQyeXVlMFlHSDYyV1hYUUhyOVNVREgrRlowVnQvRGZsdklVTWRj
                UUFEZjM4aU9zCjlQbG1kb3YrcE0vNCs5a1h5aDhSUEkzZXZ6OS9NQT09Ci0tLS0tRU5EIENFUlRJ
                RklDQVRFLS0tLS0K
              # In this example, the key data is not a real PEM-encoded private key
              tls.key: |
                RXhhbXBsZSBkYXRhIGZvciB0aGUgVExTIGNydCBmaWVsZA==
        "#,
        )
        .await;

        assert_json_diff::assert_json_eq!(
            unit.secrets,
            serde_json::json!([{
                "apiVersion": "v1",
                "kind": "Secret",
                "metadata": {
                    "name": "secret-tls",
                    "namespace": "master",
                    "labels": {
                        APP_NAME_LABEL: "master"
                    }
                },
                "type": "kubernetes.io/tls",
                "data": {
                    "tls.crt": "LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCk1JSUNVakNDQWJzQ0FnMytNQTBHQ1NxR1NJYjNEUUVCQlFVQU1JR2JNUXN3Q1FZRFZRUUdFd0pLVURFT01Bd0cKQTFVRUNCTUZWRzlyZVc4eEVEQU9CZ05WQkFjVEIwTm9kVzh0YTNVeEVUQVBCZ05WQkFvVENFWnlZVzVyTkVSRQpNUmd3RmdZRFZRUUxFdzlYWldKRFpYSjBJRk4xY0hCdmNuUXhHREFXQmdOVkJBTVREMFp5WVc1ck5FUkVJRmRsCllpQkRRVEVqTUNFR0NTcUdTSWIzRFFFSkFSWVVjM1Z3Y0c5eWRFQm1jbUZ1YXpSa1pDNWpiMjB3SGhjTk1UTXcKTVRFeE1EUTFNVE01V2hjTk1UZ3dNVEV3TURRMU1UTTVXakJMTVFzd0NRWURWUVFHREFKS1VERVBNQTBHQTFVRQpDQXdHWEZSdmEzbHZNUkV3RHdZRFZRUUtEQWhHY21GdWF6UkVSREVZTUJZR0ExVUVBd3dQZDNkM0xtVjRZVzF3CmJHVXVZMjl0TUlHYU1BMEdDU3FHU0liM0RRRUJBUVVBQTRHSUFEQ0JoQUo5WThFaUhmeHhNL25PbjJTbkkxWHgKRHdPdEJEVDFKRjBReTliMVlKanV2YjdjaTEwZjVNVm1UQllqMUZTVWZNOU1vejJDVVFZdW4yRFljV29IcFA4ZQpqSG1BUFVrNVd5cDJRN1ArMjh1bklIQkphVGZlQ09PekZSUFY2MEdTWWUzNmFScG04L3dVVm16eGFLOGtCOWVaCmhPN3F1TjdtSWQxL2pWcTNKODhDQXdFQUFUQU5CZ2txaGtpRzl3MEJBUVVGQUFPQmdRQU1meTQzeE15OHh3QTUKVjF2T2NSOEtyNWNaSXdtbFhCUU8xeFEzazlxSGtyNFlUY1JxTVQ5WjVKTm1rWHYxK2VSaGcwTi9WMW5NUTRZRgpnWXcxbnlESnBnOTduZUV4VzQyeXVlMFlHSDYyV1hYUUhyOVNVREgrRlowVnQvRGZsdklVTWRjUUFEZjM4aU9zCjlQbG1kb3YrcE0vNCs5a1h5aDhSUEkzZXZ6OS9NQT09Ci0tLS0tRU5EIENFUlRJRklDQVRFLS0tLS0K",
                    "tls.key": "RXhhbXBsZSBkYXRhIGZvciB0aGUgVExTIGNydCBmaWVsZA=="
                }
            }])
        )
    }

    #[tokio::test]
    async fn parse_unit_from_deployment_stdout() {
        let unit = parse_unit(
            r#"
            apiVersion: apps/v1
            kind: Deployment
            metadata:
              name: nginx-deployment
              labels:
                app: nginx
            spec:
              selector:
                matchLabels:
                  app: nginx
              template:
                metadata:
                  labels:
                    app: nginx
                spec:
                  containers:
                  - name: nginx
                    image: nginx:1.14.2
                    ports:
                    - containerPort: 80
                        "#,
        )
        .await;

        assert_json_diff::assert_json_eq!(
            unit.deployments,
            serde_json::json!([{
                "apiVersion": "apps/v1",
                "kind": "Deployment",
                "metadata": {
                    "name": "nginx-deployment",
                    "namespace": "master",
                    "labels": {
                        "app": "nginx",
                        APP_NAME_LABEL: "master",
                        SERVICE_NAME_LABEL: "nginx-deployment",
                        CONTAINER_TYPE_LABEL: "app-companion"
                    }
                },
                "spec": {
                    "selector": {
                        "matchLabels": {
                            "app": "nginx"
                        }
                    },
                    "template": {
                        "metadata": {
                            "labels": {
                                "app": "nginx"
                            }
                        },
                        "spec": {
                            "containers": [{
                                "name": "nginx",
                                "image": "nginx:1.14.2",
                                "ports": [{
                                    "containerPort": 80
                                }]
                            }]
                        }
                    }
                }
            }])
        )
    }

    #[tokio::test]
    async fn merge_deployment_into_bootstrapped_deployment() {
        let mut unit = parse_unit(
            r#"
            apiVersion: apps/v1
            kind: Deployment
            metadata:
              name: nginx
              labels:
                app: nginx
            spec:
              selector:
                matchLabels:
                  app: nginx
              template:
                metadata:
                  labels:
                    app: nginx
                spec:
                  containers:
                  - name: nginx
                    image: nginx:1.14.2
                    ports:
                    - containerPort: 80
                        "#,
        )
        .await;

        unit.merge(
            None,
            Service {
                ..Default::default()
            },
            Deployment {
                metadata: ObjectMeta {
                    name: Some(String::from("random-name")),
                    labels: Some(BTreeMap::from([
                        (SERVICE_NAME_LABEL.to_string(), String::from("nginx")),
                        (CONTAINER_TYPE_LABEL.to_string(), String::from("instance")),
                    ])),
                    annotations: Some(BTreeMap::from([(
                        String::from("my-important-annotation"),
                        String::from("test data"),
                    )])),
                    ..Default::default()
                },
                spec: Some(DeploymentSpec {
                    selector: LabelSelector {
                        match_labels: Some(BTreeMap::from([(
                            SERVICE_NAME_LABEL.to_string(),
                            String::from("random-name"),
                        )])),
                        ..Default::default()
                    },
                    template: PodTemplateSpec {
                        metadata: Some(ObjectMeta {
                            annotations: Some(BTreeMap::from([(
                                String::from("date"),
                                String::from("2024-01-01"),
                            )])),
                            ..Default::default()
                        }),
                        spec: Some(PodSpec {
                            containers: vec![Container {
                                name: String::from("random-name"),
                                image: Some(String::from("nginx:1.29.0")),
                                env: Some(vec![EnvVar {
                                    name: String::from("NGINX_HOST"),
                                    value: Some(String::from("example.com")),
                                    ..Default::default()
                                }]),
                                ports: Some(vec![ContainerPort {
                                    container_port: 4711,
                                    ..Default::default()
                                }]),
                                ..Default::default()
                            }],
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Default::default()
            },
            TraefikIngressRoute {
                metadata: Default::default(),
                spec: Default::default(),
            },
            Vec::new(),
        );

        assert!(unit.secrets.is_empty());
        assert!(unit.services.is_empty());
        assert!(unit.traefik_ingresses.is_empty());
        assert!(unit.traefik_middlewares.is_empty());
        assert_json_diff::assert_json_eq!(
            unit.deployments,
            serde_json::json!([{
                "apiVersion": "apps/v1",
                "kind": "Deployment",
                "metadata": {
                    "name": "nginx",
                    "namespace": "master",
                    "labels": {
                        "app": "nginx",
                        APP_NAME_LABEL: "master",
                        SERVICE_NAME_LABEL: "nginx",
                        CONTAINER_TYPE_LABEL: "instance"
                    },
                    "annotations": {
                        "my-important-annotation": "test data"
                    }
                },
                "spec": {
                    "selector": {
                        "matchLabels": {
                            "app": "nginx"
                        }
                    },
                    "template": {
                        "metadata": {
                            "labels": {
                                "app": "nginx"
                            },
                            "annotations": {
                                "date": "2024-01-01"
                            }
                        },
                        "spec": {
                            "containers": [{
                                "name": "nginx",
                                "image": "nginx:1.29.0",
                                "env": [{
                                    "name": "NGINX_HOST",
                                    "value": "example.com"
                                }],
                                "ports": [{
                                    "containerPort": 80
                                }]
                            }]
                        }
                    }
                }
            }])
        )
    }

    #[tokio::test]
    async fn filter_by_instances_and_replicas() {
        let mut unit = parse_unit(
            r#"
            apiVersion: apps/v1
            kind: Deployment
            metadata:
              name: nginx
              labels:
                app: nginx
            spec:
              selector:
                matchLabels:
                  app: nginx
              template:
                metadata:
                  labels:
                    app: nginx
                spec:
                  containers:
                  - name: nginx
                    image: nginx:1.14.2
                    ports:
                    - containerPort: 80
                        "#,
        )
        .await;

        unit.filter_by_instances_and_replicas(std::iter::once(
            ServiceBuilder::new()
                .app_name(AppName::master().to_string())
                .id(String::from("test"))
                .config(crate::sc!("nginx", "nginx:1.15"))
                .build()
                .unwrap(),
        ));

        assert!(unit.deployments.is_empty());
    }

    #[tokio::test]
    async fn filter_not_by_instances_and_replicas() {
        let mut unit = parse_unit(
            r#"
            apiVersion: apps/v1
            kind: Deployment
            metadata:
              name: nginx
              labels:
                app: nginx
            spec:
              selector:
                matchLabels:
                  app: nginx
              template:
                metadata:
                  labels:
                    app: nginx
                spec:
                  containers:
                  - name: nginx
                    image: nginx:1.14.2
                    ports:
                    - containerPort: 80
                        "#,
        )
        .await;

        unit.filter_by_instances_and_replicas(std::iter::once(
            ServiceBuilder::new()
                .app_name(AppName::master().to_string())
                .id(String::from("test"))
                .config(crate::sc!("postgres", "postgres"))
                .build()
                .unwrap(),
        ));

        assert!(!unit.deployments.is_empty());
    }
}
