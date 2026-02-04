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
        networking::v1::{Ingress, NetworkPolicy},
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
use log::{debug, error, trace, warn};
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
    policies: Vec<NetworkPolicy>,
    traefik_ingresses: Vec<TraefikIngressRoute>,
    traefik_middlewares: Vec<TraefikMiddleware>,
}

macro_rules! parse_from_dynamic_object {
    (
        $roles:ident,
        $role_bindings:ident,
        $stateful_sets:ident,
        $config_maps:ident,
        $secrets:ident,
        $pvcs:ident,
        $services:ident,
        $pods:ident,
        $deployments:ident,
        $jobs:ident,
        $service_accounts:ident,
        $ingresses:ident,
        $policies:ident,
        $traefik_ingresses:ident,
        $traefik_middlewares:ident,
        $api_version:ident,
        $kind:ident,
        $app_name:ident,
        $dyn_obj:ident
    ) => {
        match ($api_version, $kind) {
            (Role::API_VERSION, Role::KIND) => match $dyn_obj.clone().try_parse::<Role>()
            {
                Ok(role) => {
                    $roles.push(role);
                }
                Err(e) => {
                    error!("Cannot parse {:?} as Role: {e}", $dyn_obj.metadata.name);
                }
            },

            (RoleBinding::API_VERSION, RoleBinding::KIND) => {
                match $dyn_obj.clone().try_parse::<RoleBinding>() {
                    Ok(role_binding) => {
                        $role_bindings.push(role_binding);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as RoleBinding: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (StatefulSet::API_VERSION, StatefulSet::KIND) => {
                match $dyn_obj.clone().try_parse::<StatefulSet>() {
                    Ok(stateful_set) => {
                        $stateful_sets.push(stateful_set);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as StatefulSet: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (ConfigMap::API_VERSION, ConfigMap::KIND) => {
                match $dyn_obj.clone().try_parse::<ConfigMap>() {
                    Ok(config_map) => {
                        $config_maps.push(config_map);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as ConfigMap: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (Secret::API_VERSION, Secret::KIND) => {
                if let serde_json::Value::Object(obj) = &mut $dyn_obj.data {
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

                match $dyn_obj.clone().try_parse::<Secret>() {
                    Ok(secret) => {
                        $secrets.push(secret);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as Secret: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (PersistentVolumeClaim::API_VERSION, PersistentVolumeClaim::KIND) => {
                match $dyn_obj.clone().try_parse::<PersistentVolumeClaim>() {
                    Ok(pvc) => {
                        $pvcs.push(pvc);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as PersistentVolumeClaim: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (Service::API_VERSION, Service::KIND) => {
                match $dyn_obj.clone().try_parse::<Service>() {
                    Ok(service) => {
                        $services.push(service);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as Service: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (Deployment::API_VERSION, Deployment::KIND) => {
                match $dyn_obj.clone().try_parse::<Deployment>() {
                    Ok(mut deployment) => {
                        let service_name = deployment
                            .labels()
                            .get("app.kubernetes.io/component")
                            .cloned()
                            .unwrap_or_else(|| {
                                deployment.metadata.name.clone().unwrap_or_default()
                            });

                        deployment.labels_mut().entry(SERVICE_NAME_LABEL.to_string())
                            .or_insert(service_name);
                        deployment.labels_mut().entry(CONTAINER_TYPE_LABEL.to_string())
                            .or_insert(ContainerType::ApplicationCompanion.to_string());

                        $deployments.push(deployment);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as Deployment: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (Pod::API_VERSION, Pod::KIND) => match $dyn_obj.clone().try_parse::<Pod>() {
                Ok(pod) => {
                    $pods.push(pod);
                }
                Err(e) => {
                    error!("Cannot parse {:?} as Pod: {e}", $dyn_obj.metadata.name);
                }
            },
            (Job::API_VERSION, Job::KIND) => match $dyn_obj.clone().try_parse::<Job>() {
                Ok(job) => {
                    $jobs.push(job);
                }
                Err(e) => {
                    error!("Cannot parse {:?} as Job: {e}", $dyn_obj.metadata.name);
                }
            },
            (ServiceAccount::API_VERSION, ServiceAccount::KIND) => {
                match $dyn_obj.clone().try_parse::<ServiceAccount>() {
                    Ok(service_account) => {
                        $service_accounts.push(service_account);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as ServiceAccount: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (Ingress::API_VERSION, Ingress::KIND) => {
                match $dyn_obj.clone().try_parse::<Ingress>() {
                    Ok(ingress) => {
                        $ingresses.push(ingress);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as Ingress: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            (NetworkPolicy::API_VERSION, NetworkPolicy::KIND) => {
                match $dyn_obj.clone().try_parse::<NetworkPolicy>() {
                    Ok(policy) => {
                        $policies.push(policy);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as NetworkPolicy: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            ("traefik.containo.us/v1alpha1", "Middleware") => {
                match $dyn_obj.clone().try_parse::<TraefikMiddleware>() {
                    Ok(middleware) => {
                        $traefik_middlewares.push(middleware);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as Traefik middleware: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            ("traefik.containo.us/v1alpha1", "IngressRoute") => {
                match $dyn_obj.clone().try_parse::<TraefikIngressRoute>() {
                    Ok(ingress) => {
                        $traefik_ingresses.push(ingress);
                    }
                    Err(e) => {
                        error!(
                            "Cannot parse {:?} as Traefik ingress route: {e}",
                            $dyn_obj.metadata.name
                        );
                    }
                }
            }
            _ => {
                warn!(
                    "Cannot parse {name} ({api_version}, {kind}) for {app_name} because its kind is unknown",
                    api_version = $api_version,
                    kind = $kind,
                    app_name = $app_name,
                    name = $dyn_obj.metadata.name.unwrap_or_default()
                );
            }
        }
    };
}

macro_rules! empty_read_only_fields {
    ($field:expr) => {
        for meta in $field.iter_mut().map(|manifest| &mut manifest.metadata) {
            meta.creation_timestamp = None;
            meta.deletion_grace_period_seconds = None;
            meta.deletion_timestamp = None;
            meta.generation = None;
            meta.resource_version = None;
            meta.uid = None;
        }
    };
    ($field:expr, $( $additional_field:ident ),* ) => {
        empty_read_only_fields!($field);

        for manifest in $field.iter_mut() {
            $( manifest.$additional_field = None; )*
        }
    };
    ($field:expr, $( $additional_field:ident ),* (spec => $( $additional_field_in_spec:ident ),*)) => {
        empty_read_only_fields!($field, $( $additional_field )*);

        for manifest in $field.iter_mut() {
            if let Some(spec) = manifest.spec.as_mut() {
                $( spec.$additional_field_in_spec = None; )*
            }
        }
    }
}

impl K8sDeploymentUnit {
    async fn start_bootstrapping_pods(
        app_name: &AppName,
        client: Client,
        bootstrapping_containers: &[BootstrappingContainer],
        image_pull_secret: Option<Secret>,
    ) -> Result<(String, Vec<impl AsyncBufReadExt + use<>>)> {
        let image_pull_secrets = match image_pull_secret {
            Some(image_pull_secret) => {
                let image_pull_secrets = vec![LocalObjectReference {
                    name: image_pull_secret.metadata.name.clone().unwrap_or_default(),
                }];
                create_or_patch(client.clone(), app_name, image_pull_secret).await?;
                Some(image_pull_secrets)
            }
            None => None,
        };

        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "Bootstrapping {app_name} with {}",
                bootstrapping_containers
                    .iter()
                    .map(|bc| bc.image.to_string())
                    .reduce(|acc, s| format!("{acc}.{s}"))
                    .unwrap_or_default()
            );
        }

        let containers = bootstrapping_containers
            .iter()
            .enumerate()
            .map(|(i, bc)| {
                Ok(Container {
                    name: format!("bootstrap-{i}"),
                    image: Some(bc.image.to_string()),
                    image_pull_policy: Some(bc.image_pull_policy.to_string()),
                    args: Some(bc.args.clone()),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, RenderError>>()?;

        let pod_name = format!(
            "{}-bootstrap-{}",
            app_name.to_rfc1123_namespace_id(),
            // TODO pass task id
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
            if log::log_enabled!(log::Level::Trace) {
                trace!(
                    "Saw watch event for bootstrapping pod {pod_name} in {app_name}: {status:?}"
                );
            }

            if let WatchEvent::Bookmark(_bookmark) = status {
                debug!("Boot strapping pod {pod_name} for {app_name} ready.");
                break;
            }
        }

        Self::wait_for_running_pod(app_name, &pod_name, &api)
            .await
            .inspect_err(|err| {
                log::error!("Cannot bootstrap {app_name}: {err}");
            })?;

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

    async fn wait_for_running_pod(
        app_name: &AppName,
        pod_name: &str,
        api: &Api<Pod>,
    ) -> Result<()> {
        let interval = std::time::Duration::from_secs(2);
        let mut interval_timer = tokio::time::interval(interval);
        let start_time = tokio::time::Instant::now();
        let wait_timeout = std::time::Duration::from_secs(60);

        loop {
            tokio::select! {
                _ = interval_timer.tick() => {
                    let pod = api.get_status(pod_name).await?;

                    if let Some(phase) = pod.status.and_then(|status| status.phase) {
                        match phase.as_str() {
                            "Running" | "Succeeded" => {
                                return Ok(());
                            }
                            "Failed" | "Unknown" => {
                                return Err(KubernetesInfrastructureError::BootstrapContainerFailed {
                                    pod_name: pod_name.to_string(),
                                    app_name: app_name.clone(),
                                }
                                .into());
                            }
                            phase => {
                                if log::log_enabled!(log::Level::Trace) {
                                    trace!("Boot strapping pod {pod_name} for {app_name} still not in running phase. Currently in {phase}.");
                                }
                            }
                        }
                    }
                }
                _ = tokio::time::sleep_until(start_time + wait_timeout) => {
                    log::debug!("Timeout for bootstrapping the application {app_name} reached, stopping querying the pod status");
                    return Err(KubernetesInfrastructureError::BootstrapContainerFailed {
                        pod_name: pod_name.to_string(),
                        app_name: app_name.clone(),
                    }
                    .into());
                }
            }
        }
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
        let mut policies = Vec::new();
        let mut traefik_ingresses = Vec::new();
        let mut traefik_middlewares = Vec::new();

        for mut log_stream in log_streams.into_iter() {
            let mut stdout = String::new();
            log_stream.read_to_string(&mut stdout).await?;

            if log::log_enabled!(log::Level::Trace) {
                trace!(
                    "Received YAML from bootstrapping container in {app_name}: {}…",
                    stdout.lines().next().unwrap_or(&stdout)
                );
            }

            for doc in serde_norway::Deserializer::from_str(&stdout) {
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

                        if log::log_enabled!(log::Level::Trace) {
                            trace!(
                                "Parsed {} ({api_version}, {kind}) for {app_name} as a bootstrap application element.",
                                dy.metadata.name.as_deref().unwrap_or_default(),
                            );
                        }

                        parse_from_dynamic_object!(
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
                            ingresses,
                            policies,
                            traefik_ingresses,
                            traefik_middlewares,
                            api_version,
                            kind,
                            app_name,
                            dy
                        );
                    }
                    Err(err) => {
                        warn!(
                            "The output of a bootstrap container for {app_name} could not be parsed: {stdout}"
                        );
                        return Err(err.into());
                    }
                }
            }
        }

        for ingress in ingresses {
            let (route, middlewares) = match convert_k8s_ingress_to_traefik_ingress(
                ingress,
                deployment_unit.app_base_route().clone(),
                &services,
            ) {
                Ok((route, middlewares)) => (route, middlewares),
                Err((ingress, err)) => {
                    warn!(
                        "Cannot convert K8s ingress to Traefik ingress and middlewares for {app_name}: {err} ({})",
                        serde_json::to_string(&ingress).unwrap()
                    );
                    continue;
                }
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
            policies,
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
        <S as Iterator>::Item: Borrow<crate::models::Service>,
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
            name: image_pull_secret.metadata.name.clone().unwrap_or_default(),
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

    pub(super) async fn fetch(client: Client, app_name: &AppName) -> Result<Self> {
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
        let mut traefik_ingresses = Vec::new();
        let mut traefik_middlewares = Vec::new();
        let mut policies = Vec::new();

        let namespace = app_name.to_rfc1123_namespace_id();

        let api = Api::<Role>::namespaced(client, &namespace);
        roles.extend(api.list(&Default::default()).await?.items);

        let api = Api::<RoleBinding>::namespaced(api.into_client(), &namespace);
        role_bindings.extend(api.list(&Default::default()).await?.items);

        let api = Api::<StatefulSet>::namespaced(api.into_client(), &namespace);
        stateful_sets.extend(api.list(&Default::default()).await?.items);

        let api = Api::<ConfigMap>::namespaced(api.into_client(), &namespace);
        config_maps.extend(api.list(&Default::default()).await?.items);

        let api = Api::<Secret>::namespaced(api.into_client(), &namespace);
        secrets.extend(api.list(&Default::default()).await?.items);

        let api = Api::<PersistentVolumeClaim>::namespaced(api.into_client(), &namespace);
        pvcs.extend(api.list(&Default::default()).await?.items);

        let api = Api::<Service>::namespaced(api.into_client(), &namespace);
        services.extend(api.list(&Default::default()).await?.items);

        let api = Api::<Deployment>::namespaced(api.into_client(), &namespace);
        deployments.extend(api.list(&Default::default()).await?.items);

        let api = Api::<Pod>::namespaced(api.into_client(), &namespace);
        pods.extend(api.list(&Default::default()).await?.items);

        let api = Api::<Job>::namespaced(api.into_client(), &namespace);
        jobs.extend(api.list(&Default::default()).await?.items);

        let api = Api::<ServiceAccount>::namespaced(api.into_client(), &namespace);
        service_accounts.extend(api.list(&Default::default()).await?.items);

        let api = Api::<TraefikIngressRoute>::namespaced(api.into_client(), &namespace);
        traefik_ingresses.extend(api.list(&Default::default()).await?.items);

        let api = Api::<TraefikMiddleware>::namespaced(api.into_client(), &namespace);
        traefik_middlewares.extend(api.list(&Default::default()).await?.items);

        let api = Api::<NetworkPolicy>::namespaced(api.into_client(), &namespace);
        policies.extend(api.list(&Default::default()).await?.items);

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
            policies,
            traefik_ingresses,
            traefik_middlewares,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
            && self.role_bindings.is_empty()
            && self.stateful_sets.is_empty()
            && self.config_maps.is_empty()
            && self.secrets.is_empty()
            && self.pvcs.is_empty()
            && self.services.is_empty()
            && self.pods.is_empty()
            && self.deployments.is_empty()
            && self.jobs.is_empty()
            && self.service_accounts.is_empty()
            && self.policies.is_empty()
            && self.traefik_ingresses.is_empty()
            && self.traefik_middlewares.is_empty()
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
        for policy in self.policies {
            create_or_patch(client.clone(), app_name, policy).await?;
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

    pub(super) async fn delete(self, client: Client, app_name: &AppName) -> Result<()> {
        let namespace = app_name.to_rfc1123_namespace_id();

        let api = Api::<Role>::namespaced(client, &namespace);
        for role in self.roles {
            api.delete(
                role.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<RoleBinding>::namespaced(api.into_client(), &namespace);
        for role_binding in self.role_bindings {
            api.delete(
                role_binding.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<ConfigMap>::namespaced(api.into_client(), &namespace);
        for config_map in self.config_maps {
            api.delete(
                config_map.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<Secret>::namespaced(api.into_client(), &namespace);
        for secret in self.secrets {
            api.delete(
                secret.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<PersistentVolumeClaim>::namespaced(api.into_client(), &namespace);
        for pvc in self.pvcs {
            api.delete(
                pvc.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<Service>::namespaced(api.into_client(), &namespace);
        for service in self.services {
            api.delete(
                service.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<ServiceAccount>::namespaced(api.into_client(), &namespace);
        for service_account in self.service_accounts {
            api.delete(
                service_account
                    .metadata()
                    .name
                    .as_deref()
                    .unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<NetworkPolicy>::namespaced(api.into_client(), &namespace);
        for policy in self.policies {
            api.delete(
                policy.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<Deployment>::namespaced(api.into_client(), &namespace);
        for deployment in self.deployments {
            api.delete(
                deployment.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<Job>::namespaced(api.into_client(), &namespace);
        for job in self.jobs {
            api.delete(
                job.metadata().name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<StatefulSet>::namespaced(api.into_client(), &namespace);
        for stateful_set in self.stateful_sets {
            api.delete(
                stateful_set.metadata.name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<TraefikIngressRoute>::namespaced(api.into_client(), &namespace);
        for ingress in self.traefik_ingresses {
            api.delete(
                ingress.metadata.name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<TraefikMiddleware>::namespaced(api.into_client(), &namespace);
        for middleware in self.traefik_middlewares {
            api.delete(
                middleware.metadata.name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        let api = Api::<Pod>::namespaced(api.into_client(), &namespace);
        for pod in self.pods {
            api.delete(
                pod.metadata.name.as_deref().unwrap_or_default(),
                &Default::default(),
            )
            .await?;
        }

        Ok(())
    }

    /// Clears out any Kubernetes object that shouldn't be put into the backup. For example,
    /// [persistent volumes](https://kubernetes.io/docs/concepts/storage/persistent-volumes/) are
    /// removed from `self` and then `self` can be deleted from the infrastructure with
    /// [`Self::delete`].
    pub(super) fn prepare_for_back_up(mut self) -> Self {
        // Keep only the pods that aren't created by a deployment
        self.pods.retain(|pod| {
            self.deployments.iter().any(|deployment| {
                let Some(spec) = deployment.spec.as_ref() else {
                    return false;
                };
                let Some(matches_labels) = spec.selector.match_labels.as_ref() else {
                    return false;
                };

                pod.metadata
                    .labels
                    .as_ref()
                    .map(|labels| labels.iter().all(|(k, v)| matches_labels.get(k) == Some(v)))
                    .unwrap_or(false)
            })
        });

        empty_read_only_fields!(self.roles);
        empty_read_only_fields!(self.role_bindings);

        empty_read_only_fields!(self.stateful_sets, status);

        // Clear the volume mounts and keep them on the Kubernetes infrastructure because they
        // might contain data that a tester of the application crafted for a long time and this
        // should be preserved.
        self.config_maps.clear();
        self.secrets.clear();
        self.pvcs.clear();

        empty_read_only_fields!(self.services,
            status (spec => cluster_ip, cluster_ips)
        );
        empty_read_only_fields!(self.pods, status);
        empty_read_only_fields!(self.deployments, status);

        // The jobs won't be contained in the back-up because “Jobs represent one-off tasks that
        // run to completion and then stop.” If they are part of the back-up they would restart and
        // try to the same thing again which might be already done.
        self.jobs.clear();

        empty_read_only_fields!(self.service_accounts);
        empty_read_only_fields!(self.policies);
        empty_read_only_fields!(self.traefik_middlewares);
        empty_read_only_fields!(self.traefik_ingresses);

        self
    }

    pub fn parse_from_json<'a, I>(app_name: &AppName, payload: I) -> Result<Self>
    where
        I: IntoIterator,
        <I as IntoIterator>::Item: serde::de::Deserializer<'a>,
    {
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
        let mut policies = Vec::new();
        let mut traefik_middlewares = Vec::new();
        let mut traefik_ingresses = Vec::new();

        for pa in payload.into_iter() {
            match DynamicObject::deserialize(pa) {
                Ok(mut dyn_obj) => {
                    let api_version = dyn_obj
                        .types
                        .as_ref()
                        .map(|t| t.api_version.as_str())
                        .unwrap_or_default();
                    let kind = dyn_obj
                        .types
                        .as_ref()
                        .map(|t| t.kind.as_str())
                        .unwrap_or_default();

                    parse_from_dynamic_object!(
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
                        ingresses,
                        policies,
                        traefik_ingresses,
                        traefik_middlewares,
                        api_version,
                        kind,
                        app_name,
                        dyn_obj
                    );
                }
                Err(err) => anyhow::bail!("{err}"),
            }
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
            policies,
            traefik_ingresses,
            traefik_middlewares,
        })
    }

    pub fn to_json_vec(&self) -> Vec<serde_json::Value> {
        let mut json = Vec::with_capacity(
            self.roles.len()
                + self.config_maps.len()
                + self.secrets.len()
                + self.pvcs.len()
                + self.services.len()
                + self.service_accounts.len()
                + self.policies.len()
                + self.deployments.len()
                + self.jobs.len()
                + self.stateful_sets.len()
                + self.traefik_ingresses.len()
                + self.traefik_middlewares.len()
                + self.pods.len(),
        );

        for role in self.roles.iter() {
            json.push(serde_json::to_value(role).unwrap());
        }
        for config_map in self.config_maps.iter() {
            json.push(serde_json::to_value(config_map).unwrap());
        }
        for secret in self.secrets.iter() {
            json.push(serde_json::to_value(secret).unwrap());
        }
        for pvc in self.pvcs.iter() {
            json.push(serde_json::to_value(pvc).unwrap());
        }
        for service in self.services.iter() {
            json.push(serde_json::to_value(service).unwrap());
        }
        for service_account in self.service_accounts.iter() {
            json.push(serde_json::to_value(service_account).unwrap());
        }
        for policy in self.policies.iter() {
            json.push(serde_json::to_value(policy).unwrap());
        }
        for deployment in self.deployments.iter() {
            json.push(serde_json::to_value(deployment).unwrap());
        }
        for job in self.jobs.iter() {
            json.push(serde_json::to_value(job).unwrap());
        }
        for stateful_set in self.stateful_sets.iter() {
            json.push(serde_json::to_value(stateful_set).unwrap());
        }
        for ingress in self.traefik_ingresses.iter() {
            json.push(serde_json::to_value(ingress).unwrap());
        }
        for middleware in self.traefik_middlewares.iter() {
            json.push(serde_json::to_value(middleware).unwrap());
        }
        for pod in self.pods.iter() {
            json.push(serde_json::to_value(pod).unwrap());
        }

        json
    }
}

async fn create_or_patch<T>(client: Client, app_name: &AppName, payload: T) -> Result<T>
where
    T: serde::Serialize + Clone + std::fmt::Debug + for<'a> serde::Deserialize<'a>,
    T: kube::core::Resource<Scope = kube::core::NamespaceResourceScope>,
    <T as kube::Resource>::DynamicType: std::default::Default,
{
    if log::log_enabled!(log::Level::Trace) {
        trace!(
            "Create or patch {} for  {app_name}",
            payload.meta().name.as_deref().unwrap_or_default()
        );
    }

    let api = Api::namespaced(client.clone(), &app_name.to_rfc1123_namespace_id());
    match api.create(&PostParams::default(), &payload).await {
        Ok(result) => Ok(result),
        Err(kube::error::Error::Api(kube::error::ErrorResponse { code: 409, .. })) => {
            let name = payload.meta().name.as_deref().unwrap_or_default();
            match api
                .patch(name, &PatchParams::default(), &Patch::Merge(&payload))
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
    use crate::{deployment::deployment_unit::DeploymentUnitBuilder, models::State};
    use assert_json_diff::assert_json_include;
    use chrono::Utc;
    use k8s_openapi::api::{
        apps::v1::DeploymentSpec,
        core::v1::{ContainerPort, EnvVar, PodTemplateSpec},
    };
    use std::collections::HashMap;

    async fn parse_unit_from_log_stream(stdout: &'static str) -> K8sDeploymentUnit {
        let log_streams = vec![stdout.as_bytes()];

        let deployment_unit = DeploymentUnitBuilder::init(AppName::master(), Vec::new())
            .extend_with_config(&Default::default())
            .extend_with_templating_only_service_configs(Vec::new())
            .extend_with_image_infos(HashMap::new())
            .without_owners()
            .apply_templating(&None, None)
            .unwrap()
            .apply_hooks(&Default::default())
            .await
            .unwrap()
            .apply_base_traefik_ingress_route(
                crate::infrastructure::TraefikIngressRoute::with_app_only_defaults(
                    &AppName::master(),
                ),
            )
            .unwrap()
            .build();

        K8sDeploymentUnit::parse_from_log_streams(&deployment_unit, log_streams)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn parse_unit_from_secret_stdout_where_value_is_base64_encoded() {
        let unit = parse_unit_from_log_stream(
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
        let unit = parse_unit_from_log_stream(
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
        let mut unit = parse_unit_from_log_stream(
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
        let mut unit = parse_unit_from_log_stream(
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

        unit.filter_by_instances_and_replicas(std::iter::once(crate::models::Service {
            id: String::from("test"),
            config: crate::sc!("nginx", "nginx:1.15"),
            state: State {
                status: crate::models::ServiceStatus::Running,
                started_at: Some(Utc::now()),
            },
        }));

        assert!(unit.deployments.is_empty());
    }

    #[tokio::test]
    async fn filter_not_by_instances_and_replicas() {
        let mut unit = parse_unit_from_log_stream(
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

        unit.filter_by_instances_and_replicas(std::iter::once(crate::models::Service {
            id: String::from("test"),
            config: crate::sc!("postgres", "postgres"),
            state: State {
                status: crate::models::ServiceStatus::Running,
                started_at: Some(Utc::now()),
            },
        }));

        assert!(!unit.deployments.is_empty());
    }

    fn captured_example_from_k3s() -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "apiVersion": "v1",
                "kind": "Service",
                "metadata": {
                    "creationTimestamp": "2025-12-23T09:05:11Z",
                    "name": "blog",
                    "namespace": "test",
                    "resourceVersion": "869",
                    "uid": "a27286f6-6193-4ab7-a3a8-88f55151e625"
                },
                "spec": {
                    "clusterIP": "10.43.226.56",
                    "clusterIPs": [
                        "10.43.226.56"
                    ],
                    "internalTrafficPolicy": "Cluster",
                    "ipFamilies": [
                        "IPv4"
                    ],
                    "ipFamilyPolicy": "SingleStack",
                    "ports": [
                        {
                            "name": "blog",
                            "port": 80,
                            "protocol": "TCP",
                            "targetPort": 80
                        }
                    ],
                    "selector": {
                        "com.aixigo.preview.servant.app-name": "test",
                        "com.aixigo.preview.servant.container-type": "instance",
                        "com.aixigo.preview.servant.service-name": "blog"
                    },
                    "sessionAffinity": "None",
                    "type": "ClusterIP"
                },
                "status": {
                    "loadBalancer": {}
                }
            }),
            serde_json::json!({
                "apiVersion": "v1",
                "kind": "Service",
                "metadata": {
                    "creationTimestamp": "2025-12-23T09:05:11Z",
                    "name": "db",
                    "namespace": "test",
                    "resourceVersion": "865",
                    "uid": "ed8c87a2-632d-4cd5-9b0e-7d1cc6f08236"
                },
                "spec": {
                    "clusterIP": "10.43.177.211",
                    "clusterIPs": [
                        "10.43.177.211"
                    ],
                    "internalTrafficPolicy": "Cluster",
                    "ipFamilies": [
                        "IPv4"
                    ],
                    "ipFamilyPolicy": "SingleStack",
                    "ports": [
                        {
                            "name": "db",
                            "port": 3306,
                            "protocol": "TCP",
                            "targetPort": 3306
                        }
                    ],
                    "selector": {
                        "com.aixigo.preview.servant.app-name": "test",
                        "com.aixigo.preview.servant.container-type": "instance",
                        "com.aixigo.preview.servant.service-name": "db"
                    },
                    "sessionAffinity": "None",
                    "type": "ClusterIP"
                },
                "status": {
                    "loadBalancer": {}
                }
            }),
            serde_json::json!({
                "apiVersion": "apps/v1",
                "kind": "Deployment",
                "metadata": {
                    "annotations": {
                        "com.aixigo.preview.servant.image": "docker.io/library/wordpress:latest",
                        "com.aixigo.preview.servant.replicated-env": "{\"WORDPRESS_CONFIG_EXTRA\":{\"replicate\":true,\"templated\":true,\"value\":\"define('WP_HOME','http://localhost');\\ndefine('WP_SITEURL','http://localhost/{{application.name}}/blog');\"},\"WORDPRESS_DB_HOST\":{\"replicate\":true,\"templated\":false,\"value\":\"db\"},\"WORDPRESS_DB_NAME\":{\"replicate\":true,\"templated\":false,\"value\":\"example-database\"},\"WORDPRESS_DB_PASSWORD\":{\"replicate\":true,\"templated\":false,\"value\":\"my_cool_secret\"},\"WORDPRESS_DB_USER\":{\"replicate\":true,\"templated\":false,\"value\":\"example-user\"}}",
                        "deployment.kubernetes.io/revision": "1"
                    },
                    "creationTimestamp": "2025-12-23T09:05:11Z",
                    "generation": 1,
                    "labels": {
                        "com.aixigo.preview.servant.app-name": "test",
                        "com.aixigo.preview.servant.container-type": "instance",
                        "com.aixigo.preview.servant.service-name": "blog"
                    },
                    "name": "test-blog-deployment",
                    "namespace": "test",
                    "resourceVersion": "916",
                    "uid": "43a087ee-0368-4499-8dfc-57739afa8230"
                },
                "spec": {
                    "progressDeadlineSeconds": 600,
                    "replicas": 1,
                    "revisionHistoryLimit": 10,
                    "selector": {
                        "matchLabels": {
                            "com.aixigo.preview.servant.app-name": "test",
                            "com.aixigo.preview.servant.container-type": "instance",
                            "com.aixigo.preview.servant.service-name": "blog"
                        }
                    },
                    "strategy": {
                        "rollingUpdate": {
                            "maxSurge": "25%",
                            "maxUnavailable": "25%"
                        },
                        "type": "RollingUpdate"
                    },
                    "template": {
                        "metadata": {
                            "annotations": {
                                "date": "2025-12-23T09:05:11.886481258+00:00"
                            },
                            "labels": {
                                "com.aixigo.preview.servant.app-name": "test",
                                "com.aixigo.preview.servant.container-type": "instance",
                                "com.aixigo.preview.servant.service-name": "blog"
                            }
                        },
                        "spec": {
                            "containers": [
                                {
                                    "env": [
                                        {
                                            "name": "WORDPRESS_CONFIG_EXTRA",
                                            "value": "define('WP_HOME','http://localhost');\ndefine('WP_SITEURL','http://localhost/test/blog');"
                                        },
                                        {
                                            "name": "WORDPRESS_DB_HOST",
                                            "value": "db"
                                        },
                                        {
                                            "name": "WORDPRESS_DB_NAME",
                                            "value": "example-database"
                                        },
                                        {
                                            "name": "WORDPRESS_DB_PASSWORD",
                                            "value": "my_cool_secret"
                                        },
                                        {
                                            "name": "WORDPRESS_DB_USER",
                                            "value": "example-user"
                                        }
                                    ],
                                    "image": "docker.io/library/wordpress:latest",
                                    "imagePullPolicy": "Always",
                                    "name": "blog",
                                    "ports": [
                                        {
                                            "containerPort": 80,
                                            "protocol": "TCP"
                                        }
                                    ],
                                    "resources": {},
                                    "terminationMessagePath": "/dev/termination-log",
                                    "terminationMessagePolicy": "File"
                                }
                            ],
                            "dnsPolicy": "ClusterFirst",
                            "restartPolicy": "Always",
                            "schedulerName": "default-scheduler",
                            "securityContext": {},
                            "terminationGracePeriodSeconds": 30
                        }
                    }
                },
                "status": {
                    "availableReplicas": 1,
                    "conditions": [
                        {
                            "lastTransitionTime": "2025-12-23T09:05:14Z",
                            "lastUpdateTime": "2025-12-23T09:05:14Z",
                            "message": "Deployment has minimum availability.",
                            "reason": "MinimumReplicasAvailable",
                            "status": "True",
                            "type": "Available"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:11Z",
                            "lastUpdateTime": "2025-12-23T09:05:14Z",
                            "message": "ReplicaSet \"test-blog-deployment-5bb689bcd9\" has successfully progressed.",
                            "reason": "NewReplicaSetAvailable",
                            "status": "True",
                            "type": "Progressing"
                        }
                    ],
                    "observedGeneration": 1,
                    "readyReplicas": 1,
                    "replicas": 1,
                    "updatedReplicas": 1
                }
            }),
            serde_json::json!({
                "apiVersion": "apps/v1",
                "kind": "Deployment",
                "metadata": {
                    "annotations": {
                        "com.aixigo.preview.servant.image": "docker.io/library/mariadb:latest",
                        "com.aixigo.preview.servant.replicated-env": "{\"MARIADB_DATABASE\":{\"replicate\":true,\"templated\":false,\"value\":\"example-database\"},\"MARIADB_PASSWORD\":{\"replicate\":true,\"templated\":false,\"value\":\"my_cool_secret\"},\"MARIADB_ROOT_PASSWORD\":{\"replicate\":true,\"templated\":false,\"value\":\"example\"},\"MARIADB_USER\":{\"replicate\":true,\"templated\":false,\"value\":\"example-user\"}}",
                        "deployment.kubernetes.io/revision": "1"
                    },
                    "creationTimestamp": "2025-12-23T09:05:11Z",
                    "generation": 1,
                    "labels": {
                        "com.aixigo.preview.servant.app-name": "test",
                        "com.aixigo.preview.servant.container-type": "instance",
                        "com.aixigo.preview.servant.service-name": "db"
                    },
                    "name": "test-db-deployment",
                    "namespace": "test",
                    "resourceVersion": "920",
                    "uid": "7cae9a70-9187-442e-9700-dce0840d25e6"
                },
                "spec": {
                    "progressDeadlineSeconds": 600,
                    "replicas": 1,
                    "revisionHistoryLimit": 10,
                    "selector": {
                        "matchLabels": {
                            "com.aixigo.preview.servant.app-name": "test",
                            "com.aixigo.preview.servant.container-type": "instance",
                            "com.aixigo.preview.servant.service-name": "db"
                        }
                    },
                    "strategy": {
                        "rollingUpdate": {
                            "maxSurge": "25%",
                            "maxUnavailable": "25%"
                        },
                        "type": "RollingUpdate"
                    },
                    "template": {
                        "metadata": {
                            "annotations": {
                                "date": "2025-12-23T09:05:11.873776450+00:00"
                            },
                            "labels": {
                                "com.aixigo.preview.servant.app-name": "test",
                                "com.aixigo.preview.servant.container-type": "instance",
                                "com.aixigo.preview.servant.service-name": "db"
                            }
                        },
                        "spec": {
                            "containers": [
                                {
                                    "env": [
                                        {
                                            "name": "MARIADB_DATABASE",
                                            "value": "example-database"
                                        },
                                        {
                                            "name": "MARIADB_PASSWORD",
                                            "value": "my_cool_secret"
                                        },
                                        {
                                            "name": "MARIADB_ROOT_PASSWORD",
                                            "value": "example"
                                        },
                                        {
                                            "name": "MARIADB_USER",
                                            "value": "example-user"
                                        }
                                    ],
                                    "image": "docker.io/library/mariadb:latest",
                                    "imagePullPolicy": "Always",
                                    "name": "db",
                                    "ports": [
                                        {
                                            "containerPort": 3306,
                                            "protocol": "TCP"
                                        }
                                    ],
                                    "resources": {},
                                    "terminationMessagePath": "/dev/termination-log",
                                    "terminationMessagePolicy": "File"
                                }
                            ],
                            "dnsPolicy": "ClusterFirst",
                            "restartPolicy": "Always",
                            "schedulerName": "default-scheduler",
                            "securityContext": {},
                            "terminationGracePeriodSeconds": 30
                        }
                    }
                },
                "status": {
                    "availableReplicas": 1,
                    "conditions": [
                        {
                            "lastTransitionTime": "2025-12-23T09:05:14Z",
                            "lastUpdateTime": "2025-12-23T09:05:14Z",
                            "message": "Deployment has minimum availability.",
                            "reason": "MinimumReplicasAvailable",
                            "status": "True",
                            "type": "Available"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:11Z",
                            "lastUpdateTime": "2025-12-23T09:05:14Z",
                            "message": "ReplicaSet \"test-db-deployment-5fcf85b44b\" has successfully progressed.",
                            "reason": "NewReplicaSetAvailable",
                            "status": "True",
                            "type": "Progressing"
                        }
                    ],
                    "observedGeneration": 1,
                    "readyReplicas": 1,
                    "replicas": 1,
                    "updatedReplicas": 1
                }
            }),
            serde_json::json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "annotations": {
                        "date": "2025-12-23T09:05:11.886481258+00:00"
                    },
                    "creationTimestamp": "2025-12-23T09:05:11Z",
                    "generateName": "test-blog-deployment-5bb689bcd9-",
                    "labels": {
                        "com.aixigo.preview.servant.app-name": "test",
                        "com.aixigo.preview.servant.container-type": "instance",
                        "com.aixigo.preview.servant.service-name": "blog",
                        "pod-template-hash": "5bb689bcd9"
                    },
                    "name": "test-blog-deployment-5bb689bcd9-wj557",
                    "namespace": "test",
                    "ownerReferences": [
                        {
                            "apiVersion": "apps/v1",
                            "blockOwnerDeletion": true,
                            "controller": true,
                            "kind": "ReplicaSet",
                            "name": "test-blog-deployment-5bb689bcd9",
                            "uid": "13227d25-9fc9-46bf-b199-30137d4dfbb6"
                        }
                    ],
                    "resourceVersion": "911",
                    "uid": "9730567c-8c14-4264-a790-c347d317056d"
                },
                "spec": {
                    "containers": [
                        {
                            "env": [
                                {
                                    "name": "WORDPRESS_CONFIG_EXTRA",
                                    "value": "define('WP_HOME','http://localhost');\ndefine('WP_SITEURL','http://localhost/test/blog');"
                                },
                                {
                                    "name": "WORDPRESS_DB_HOST",
                                    "value": "db"
                                },
                                {
                                    "name": "WORDPRESS_DB_NAME",
                                    "value": "example-database"
                                },
                                {
                                    "name": "WORDPRESS_DB_PASSWORD",
                                    "value": "my_cool_secret"
                                },
                                {
                                    "name": "WORDPRESS_DB_USER",
                                    "value": "example-user"
                                }
                            ],
                            "image": "docker.io/library/wordpress:latest",
                            "imagePullPolicy": "Always",
                            "name": "blog",
                            "ports": [
                                {
                                    "containerPort": 80,
                                    "protocol": "TCP"
                                }
                            ],
                            "resources": {},
                            "terminationMessagePath": "/dev/termination-log",
                            "terminationMessagePolicy": "File",
                            "volumeMounts": [
                                {
                                    "mountPath": "/var/run/secrets/kubernetes.io/serviceaccount",
                                    "name": "kube-api-access-8cdhb",
                                    "readOnly": true
                                }
                            ]
                        }
                    ],
                    "dnsPolicy": "ClusterFirst",
                    "enableServiceLinks": true,
                    "nodeName": "k3d-dash-server-0",
                    "preemptionPolicy": "PreemptLowerPriority",
                    "priority": 0,
                    "restartPolicy": "Always",
                    "schedulerName": "default-scheduler",
                    "securityContext": {},
                    "serviceAccount": "default",
                    "serviceAccountName": "default",
                    "terminationGracePeriodSeconds": 30,
                    "tolerations": [
                        {
                            "effect": "NoExecute",
                            "key": "node.kubernetes.io/not-ready",
                            "operator": "Exists",
                            "tolerationSeconds": 300
                        },
                        {
                            "effect": "NoExecute",
                            "key": "node.kubernetes.io/unreachable",
                            "operator": "Exists",
                            "tolerationSeconds": 300
                        }
                    ],
                    "volumes": [
                        {
                            "name": "kube-api-access-8cdhb",
                            "projected": {
                                "defaultMode": 420,
                                "sources": [
                                    {
                                        "serviceAccountToken": {
                                            "expirationSeconds": 3607,
                                            "path": "token"
                                        }
                                    },
                                    {
                                        "configMap": {
                                            "items": [
                                                {
                                                    "key": "ca.crt",
                                                    "path": "ca.crt"
                                                }
                                            ],
                                            "name": "kube-root-ca.crt"
                                        }
                                    },
                                    {
                                        "downwardAPI": {
                                            "items": [
                                                {
                                                    "fieldRef": {
                                                        "apiVersion": "v1",
                                                        "fieldPath": "metadata.namespace"
                                                    },
                                                    "path": "namespace"
                                                }
                                            ]
                                        }
                                    }
                                ]
                            }
                        }
                    ]
                },
                "status": {
                    "conditions": [
                        {
                            "lastTransitionTime": "2025-12-23T09:05:14Z",
                            "status": "True",
                            "type": "PodReadyToStartContainers"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:12Z",
                            "status": "True",
                            "type": "Initialized"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:14Z",
                            "status": "True",
                            "type": "Ready"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:14Z",
                            "status": "True",
                            "type": "ContainersReady"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:11Z",
                            "status": "True",
                            "type": "PodScheduled"
                        }
                    ],
                    "containerStatuses": [
                        {
                            "containerID": "containerd://a58e137c636cd57c0ce8c392990e59d9ad638cd577cc40317cf19beea771a25c",
                            "image": "docker.io/library/wordpress:latest",
                            "imageID": "docker.io/library/wordpress@sha256:c6c44891a684b52c0d183d9d1f182dca1e16d58711670a4d973e9625d903efb3",
                            "lastState": {},
                            "name": "blog",
                            "ready": true,
                            "restartCount": 0,
                            "started": true,
                            "state": {
                                "running": {
                                    "startedAt": "2025-12-23T09:05:13Z"
                                }
                            },
                            "volumeMounts": [
                                {
                                    "mountPath": "/var/run/secrets/kubernetes.io/serviceaccount",
                                    "name": "kube-api-access-8cdhb",
                                    "readOnly": true,
                                    "recursiveReadOnly": "Disabled"
                                }
                            ]
                        }
                    ],
                    "hostIP": "172.24.25.2",
                    "hostIPs": [
                        {
                            "ip": "172.24.25.2"
                        }
                    ],
                    "phase": "Running",
                    "podIP": "10.42.0.12",
                    "podIPs": [
                        {
                            "ip": "10.42.0.12"
                        }
                    ],
                    "qosClass": "BestEffort",
                    "startTime": "2025-12-23T09:05:12Z"
                }
            }),
            serde_json::json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "annotations": {
                        "date": "2025-12-23T09:05:11.873776450+00:00"
                    },
                    "creationTimestamp": "2025-12-23T09:05:11Z",
                    "generateName": "test-db-deployment-5fcf85b44b-",
                    "labels": {
                        "com.aixigo.preview.servant.app-name": "test",
                        "com.aixigo.preview.servant.container-type": "instance",
                        "com.aixigo.preview.servant.service-name": "db",
                        "pod-template-hash": "5fcf85b44b"
                    },
                    "name": "test-db-deployment-5fcf85b44b-dcfx9",
                    "namespace": "test",
                    "ownerReferences": [
                        {
                            "apiVersion": "apps/v1",
                            "blockOwnerDeletion": true,
                            "controller": true,
                            "kind": "ReplicaSet",
                            "name": "test-db-deployment-5fcf85b44b",
                            "uid": "9b2f3c50-2af2-4b22-8b26-31cc06034769"
                        }
                    ],
                    "resourceVersion": "915",
                    "uid": "4ff6e7e8-d784-4176-bd48-1cbe8bde8acf"
                },
                "spec": {
                    "containers": [
                        {
                            "env": [
                                {
                                    "name": "MARIADB_DATABASE",
                                    "value": "example-database"
                                },
                                {
                                    "name": "MARIADB_PASSWORD",
                                    "value": "my_cool_secret"
                                },
                                {
                                    "name": "MARIADB_ROOT_PASSWORD",
                                    "value": "example"
                                },
                                {
                                    "name": "MARIADB_USER",
                                    "value": "example-user"
                                }
                            ],
                            "image": "docker.io/library/mariadb:latest",
                            "imagePullPolicy": "Always",
                            "name": "db",
                            "ports": [
                                {
                                    "containerPort": 3306,
                                    "protocol": "TCP"
                                }
                            ],
                            "resources": {},
                            "terminationMessagePath": "/dev/termination-log",
                            "terminationMessagePolicy": "File",
                            "volumeMounts": [
                                {
                                    "mountPath": "/var/run/secrets/kubernetes.io/serviceaccount",
                                    "name": "kube-api-access-ltqf9",
                                    "readOnly": true
                                }
                            ]
                        }
                    ],
                    "dnsPolicy": "ClusterFirst",
                    "enableServiceLinks": true,
                    "nodeName": "k3d-dash-server-0",
                    "preemptionPolicy": "PreemptLowerPriority",
                    "priority": 0,
                    "restartPolicy": "Always",
                    "schedulerName": "default-scheduler",
                    "securityContext": {},
                    "serviceAccount": "default",
                    "serviceAccountName": "default",
                    "terminationGracePeriodSeconds": 30,
                    "tolerations": [
                        {
                            "effect": "NoExecute",
                            "key": "node.kubernetes.io/not-ready",
                            "operator": "Exists",
                            "tolerationSeconds": 300
                        },
                        {
                            "effect": "NoExecute",
                            "key": "node.kubernetes.io/unreachable",
                            "operator": "Exists",
                            "tolerationSeconds": 300
                        }
                    ],
                    "volumes": [
                        {
                            "name": "kube-api-access-ltqf9",
                            "projected": {
                                "defaultMode": 420,
                                "sources": [
                                    {
                                        "serviceAccountToken": {
                                            "expirationSeconds": 3607,
                                            "path": "token"
                                        }
                                    },
                                    {
                                        "configMap": {
                                            "items": [
                                                {
                                                    "key": "ca.crt",
                                                    "path": "ca.crt"
                                                }
                                            ],
                                            "name": "kube-root-ca.crt"
                                        }
                                    },
                                    {
                                        "downwardAPI": {
                                            "items": [
                                                {
                                                    "fieldRef": {
                                                        "apiVersion": "v1",
                                                        "fieldPath": "metadata.namespace"
                                                    },
                                                    "path": "namespace"
                                                }
                                            ]
                                        }
                                    }
                                ]
                            }
                        }
                    ]
                },
                "status": {
                    "conditions": [
                        {
                            "lastTransitionTime": "2025-12-23T09:05:14Z",
                            "status": "True",
                            "type": "PodReadyToStartContainers"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:11Z",
                            "status": "True",
                            "type": "Initialized"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:14Z",
                            "status": "True",
                            "type": "Ready"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:14Z",
                            "status": "True",
                            "type": "ContainersReady"
                        },
                        {
                            "lastTransitionTime": "2025-12-23T09:05:11Z",
                            "status": "True",
                            "type": "PodScheduled"
                        }
                    ],
                    "containerStatuses": [
                        {
                            "containerID": "containerd://5e13883138c5dad22ea0279b005678a0dafb8d80a102f1d5cf1b5b75594600d3",
                            "image": "docker.io/library/mariadb:latest",
                            "imageID": "docker.io/library/mariadb@sha256:e1bcd6f85781f4a875abefb11c4166c1d79e4237c23de597bf0df81fec225b40",
                            "lastState": {},
                            "name": "db",
                            "ready": true,
                            "restartCount": 0,
                            "started": true,
                            "state": {
                                "running": {
                                    "startedAt": "2025-12-23T09:05:13Z"
                                }
                            },
                            "volumeMounts": [
                                {
                                    "mountPath": "/var/run/secrets/kubernetes.io/serviceaccount",
                                    "name": "kube-api-access-ltqf9",
                                    "readOnly": true,
                                    "recursiveReadOnly": "Disabled"
                                }
                            ]
                        }
                    ],
                    "hostIP": "172.24.25.2",
                    "hostIPs": [
                        {
                            "ip": "172.24.25.2"
                        }
                    ],
                    "phase": "Running",
                    "podIP": "10.42.0.13",
                    "podIPs": [
                        {
                            "ip": "10.42.0.13"
                        }
                    ],
                    "qosClass": "BestEffort",
                    "startTime": "2025-12-23T09:05:11Z"
                }
            }),
        ]
    }

    #[test]
    fn parse_from_json() {
        let json = captured_example_from_k3s();

        let unit = K8sDeploymentUnit::parse_from_json(&AppName::master(), json).unwrap();

        assert_eq!(unit.roles.len(), 0);
        assert_eq!(unit.role_bindings.len(), 0);
        assert_eq!(unit.stateful_sets.len(), 0);
        assert_eq!(unit.config_maps.len(), 0);
        assert_eq!(unit.secrets.len(), 0);
        assert_eq!(unit.pvcs.len(), 0);
        assert_eq!(unit.services.len(), 2);
        assert_eq!(unit.pods.len(), 2);
        assert_eq!(unit.deployments.len(), 2);
        assert_eq!(unit.jobs.len(), 0);
        assert_eq!(unit.service_accounts.len(), 0);
        assert_eq!(unit.policies.len(), 0);
        assert_eq!(unit.traefik_ingresses.len(), 0);
        assert_eq!(unit.traefik_middlewares.len(), 0);
    }

    #[test]
    fn to_json_vec() {
        let json = captured_example_from_k3s();

        let unit = K8sDeploymentUnit::parse_from_json(&AppName::master(), json).unwrap();

        assert_json_include!(
            actual: serde_json::Value::Array(unit.to_json_vec()),
            expected: serde_json::Value::Array(captured_example_from_k3s())
        );
    }

    mod prepare_for_back_up {
        use super::*;

        #[test]
        fn clean_system_populations() {
            let unit =
                K8sDeploymentUnit::parse_from_json(&AppName::master(), captured_example_from_k3s())
                    .unwrap();

            let prepare_for_back_up = unit.prepare_for_back_up();

            assert!(
                prepare_for_back_up.pods.is_empty(),
                "Pods should be empty because they are covered by deployments"
            );
            assert!(
                !prepare_for_back_up
                    .services
                    .iter()
                    .filter_map(|service| service.spec.as_ref())
                    .any(|spec| spec.cluster_ip.is_some() || spec.cluster_ips.is_some()),
                "Don't preserve cluster IP(s)"
            );
            assert!(
                !prepare_for_back_up
                    .deployments
                    .iter()
                    .any(|deployment| deployment.status.is_some()),
                "Deployment's status will be set by server"
            );
        }

        #[tokio::test]
        async fn clean_jobs() {
            let unit = parse_unit_from_log_stream(
                r#"
apiVersion: batch/v1
kind: Job
metadata:
  name: pi
spec:
  template:
    spec:
      containers:
      - name: pi
        image: perl:5.34.0
        command: ["perl",  "-Mbignum=bpi", "-wle", "print bpi(2000)"]
      restartPolicy: Never
  backoffLimit: 4 "#,
            )
            .await;

            assert!(!unit.jobs.is_empty());

            let prepared_for_back_up = unit.prepare_for_back_up();

            assert!(prepared_for_back_up.jobs.is_empty());
        }

        #[tokio::test]
        async fn clean_volume_mounts() {
            let unit = parse_unit_from_log_stream(
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
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: game-demo
data:
  # property-like keys; each key maps to a simple value
  player_initial_lives: "3"
  ui_properties_file_name: "user-interface.properties"

  # file-like keys
  game.properties: |
    enemy.types=aliens,monsters
    player.maximum-lives=5
  user-interface.properties: |
    color.good=purple
    color.bad=yellow
    allow.textmode=true
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: foo-pvc
  namespace: foo
spec:
  storageClassName: ""
  volumeName: foo-pv
                "#,
            )
            .await;

            assert!(!unit.secrets.is_empty());
            assert!(!unit.config_maps.is_empty());
            assert!(!unit.pvcs.is_empty());

            let prepared_for_back_up = unit.prepare_for_back_up();

            assert!(prepared_for_back_up.secrets.is_empty());
            assert!(prepared_for_back_up.config_maps.is_empty());
            assert!(prepared_for_back_up.pvcs.is_empty());
        }
    }
}
