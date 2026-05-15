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
use super::super::{
    APP_NAME_LABEL, CONTAINER_TYPE_LABEL, IMAGE_LABEL, REPLICATED_ENV_LABEL, SERVICE_NAME_LABEL,
    STORAGE_TYPE_LABEL,
};
use crate::config::{Config, ContainerConfig};
use crate::infrastructure::kubernetes::{
    infrastructure::KubernetesInfrastructureError,
    traefik_crds::{
        IngressRoute, IngressRouteSpec, Middleware, MiddlewareSpec, TraefikRuleMiddlewareRef,
        TraefikRuleService, TraefikRuleSpec, TraefikTls,
    },
};
use crate::infrastructure::{OWNERS_LABEL, USER_DEFINED_PARAMETERS_LABEL};
use base64::{Engine, engine::general_purpose};
use bytesize::ByteSize;
use chrono::Utc;
use domain::{
    AppName, Image, Owner,
    app_blueprints::{Environment, ServiceConfig, UserDefinedParameters},
    app_deployment::{DeployableService, DeploymentStrategy},
    app_instance::{ContainerType, Service, ServiceStatus},
    traefik::{TraefikIngressRoute, TraefikMiddleware, TraefikRouterRule},
};
use k8s_openapi::ByteString;
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::api::{
    apps::v1::{Deployment as V1Deployment, DeploymentSpec},
    core::v1::{
        Container, ContainerPort, EnvVar, KeyToPath, Namespace as V1Namespace,
        PersistentVolumeClaim, PersistentVolumeClaimSpec, PersistentVolumeClaimVolumeSource, Pod,
        PodSpec, PodTemplateSpec, ResourceRequirements, Secret as V1Secret, SecretVolumeSource,
        Service as V1Service, Volume, VolumeMount, VolumeResourceRequirements,
    },
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::core::ObjectMeta;
use multimap::MultiMap;
use secstr::SecUtf8;
use serde_json::{Map, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hasher;
use std::iter::FromIterator;
use std::path::Component;
use std::str::FromStr;
use std::string::ToString;

macro_rules! secret_name_from_path {
    ($path:expr_2021) => {{
        $path
            .components()
            .map(|c| match c {
                Component::Normal(c) => c.to_os_string().into_string().unwrap(),
                _ => "".to_string(),
            })
            .filter(|c| !c.is_empty())
            .map(|c| c.replace(".", "-"))
            .collect::<Vec<String>>()
            .join("-")
    }};
}

macro_rules! secret_name_from_name {
    ($path:expr_2021) => {{
        $path
            .file_name()
            .map(|name| name.to_os_string().into_string().unwrap())
            .map(|name| name.replace(".", "-"))
            .unwrap_or_else(String::new)
    }};
}

pub fn kubernetes_object_to_service(
    deployment: V1Deployment,
    pod: Option<Pod>,
) -> Result<Service, KubernetesInfrastructureError> {
    let service_config = kubernetes_deployement_to_service_config(&deployment)?;

    let name = deployment
        .metadata
        .name
        .ok_or(KubernetesInfrastructureError::DeploymentWithoutName)?;

    let started_at = pod.and_then(|pod| {
        pod.status
            .as_ref()
            .and_then(|s| s.start_time.as_ref())
            .map(|t| t.0)
    });

    let status = deployment
        .spec
        .as_ref()
        .map(|spec| match spec.replicas {
            None => ServiceStatus::Paused,
            Some(replicas) if replicas <= 0 || started_at.is_none() => ServiceStatus::Paused,
            _ => ServiceStatus::Running {
                started_at: started_at.unwrap(),
            },
        })
        .unwrap_or(ServiceStatus::Paused);

    let service_type = if let Some(lb) = deployment
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(CONTAINER_TYPE_LABEL))
    {
        lb.parse::<ContainerType>()?
    } else {
        ContainerType::Instance
    };

    Ok(Service {
        id: name,
        blueprint_config: service_config,
        status,
        service_type,
    })
}

pub fn kubernetes_deployement_to_service_config(
    deployment: &V1Deployment,
) -> Result<ServiceConfig, KubernetesInfrastructureError> {
    let deployment_name = deployment
        .metadata
        .name
        .as_ref()
        .ok_or_else(|| KubernetesInfrastructureError::DeploymentWithoutName)?;

    let service_name = deployment
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(SERVICE_NAME_LABEL))
        .unwrap_or(deployment_name);
    let image = match deployment
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(IMAGE_LABEL))
        .and_then(|image| Image::from_str(image).ok())
    {
        Some(img) => img,
        None => deployment
            .spec
            .as_ref()
            .and_then(|spec| spec.template.spec.as_ref())
            .and_then(|pod_spec| pod_spec.containers.first())
            .and_then(|container| container.image.as_ref())
            .and_then(|image| Image::from_str(image).ok())
            .ok_or_else(|| KubernetesInfrastructureError::MissingImageLabel {
                deployment_name: deployment_name.clone(),
            })?,
    };

    let mut config = ServiceConfig::new(service_name.clone(), image);

    if let Some(annotations) = &deployment.metadata.annotations {
        if let Some(replicated_env) = annotations.get(REPLICATED_ENV_LABEL) {
            let env = serde_json::from_str::<Environment>(replicated_env).map_err(|err| {
                KubernetesInfrastructureError::UnexpectedError {
                    err: anyhow::Error::new(err),
                }
            })?;
            config.env = Some(env);
        }
    }

    Ok(config)
}

#[derive(PartialEq)]
pub struct K8sSource<T>(pub T);

impl<T> std::fmt::Debug for K8sSource<T>
where
    T: kube::Resource,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("K8sSource").field(&self.0.meta()).finish()
    }
}

impl<T> std::fmt::Display for K8sSource<T>
where
    T: kube::Resource,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.meta().name.as_deref().unwrap_or_default())
    }
}

impl<T> std::error::Error for K8sSource<T> where T: kube::Resource {}

#[derive(thiserror::Error, Debug)]
pub enum ConvertK8sIngressError {
    #[error("Ingress object does not provide a name: {ingress}")]
    NoName { ingress: K8sSource<Ingress> },
    #[error("Ingress object does not provide a spec: {ingress}")]
    NoSpec { ingress: K8sSource<Ingress> },
    #[error("Ingress' spec does not provide rules: {ingress}")]
    SpecWithoutRules { ingress: K8sSource<Ingress> },
    #[error("Ingress' rule does not a provide http paths object: {ingress}")]
    RuleWithoutHttpPaths { ingress: K8sSource<Ingress> },
    #[error("Ingress' path does not provide a HTTP path value: {ingress}")]
    PathWithoutPathValue { ingress: K8sSource<Ingress> },
    #[error("Ingress' path does not provide a HTTP path value: {ingress}")]
    UnknownPathType {
        ingress: K8sSource<Ingress>,
        path_type: String,
    },
    #[error("Ingress' path does not provide a service: {ingress}")]
    NoBackendService { ingress: K8sSource<Ingress> },
    #[error("There is no service matching to the ingress' service: {ingress}")]
    NoMatchingService { ingress: K8sSource<Ingress> },
    #[error("There is no service matching to the ingress' service and port name.: {ingress}")]
    NoMatchingServicePort { ingress: K8sSource<Ingress> },
}

impl ConvertK8sIngressError {
    pub fn ingress(&self) -> &Ingress {
        match &self {
            Self::NoName { ingress } => &ingress.0,
            Self::NoSpec { ingress } => &ingress.0,
            Self::SpecWithoutRules { ingress } => &ingress.0,
            Self::RuleWithoutHttpPaths { ingress } => &ingress.0,
            Self::PathWithoutPathValue { ingress } => &ingress.0,
            Self::UnknownPathType { ingress, .. } => &ingress.0,
            Self::NoBackendService { ingress } => &ingress.0,
            Self::NoMatchingService { ingress } => &ingress.0,
            Self::NoMatchingServicePort { ingress } => &ingress.0,
        }
    }
}

pub fn convert_k8s_ingress_to_traefik_rule(
    ingress: Ingress,
) -> Result<TraefikIngressRoute, Box<ConvertK8sIngressError>> {
    let Some(name) = ingress.metadata.name.as_ref() else {
        return Err(Box::new(ConvertK8sIngressError::NoName {
            ingress: K8sSource(ingress),
        }));
    };
    let Some(spec) = ingress.spec.as_ref() else {
        return Err(Box::new(ConvertK8sIngressError::NoSpec {
            ingress: K8sSource(ingress),
        }));
    };

    let Some(rules) = spec.rules.as_ref() else {
        return Err(Box::new(ConvertK8sIngressError::SpecWithoutRules {
            ingress: K8sSource(ingress),
        }));
    };

    let Some(path) = rules
        .iter()
        .filter_map(|rule| rule.http.as_ref())
        .find_map(|http| http.paths.first())
    else {
        return Err(Box::new(ConvertK8sIngressError::RuleWithoutHttpPaths {
            ingress: K8sSource(ingress),
        }));
    };

    let Some(path_value) = path.path.as_ref() else {
        return Err(Box::new(ConvertK8sIngressError::PathWithoutPathValue {
            ingress: K8sSource(ingress),
        }));
    };

    match &spec.ingress_class_name {
        Some(ingress_class_name) if ingress_class_name == "nginx" => {
            let rule = match path.path_type.as_str() {
                "Prefix" => TraefikRouterRule::path_prefix_rule([path_value.clone()]),
                "" => TraefikRouterRule::path_prefix_rule(std::iter::empty::<&'static str>()),
                path_type => {
                    return Err(Box::new(ConvertK8sIngressError::UnknownPathType {
                        ingress: K8sSource(ingress.clone()),
                        path_type: path_type.to_string(),
                    }));
                }
            };

            let middleware = ingress
                .metadata
                .annotations
                .as_ref()
                .filter(|annotations| {
                    annotations.get("nginx.ingress.kubernetes.io/use-regex")
                        == Some(&String::from("true"))
                })
                .and_then(|annotations| {
                    annotations
                        .get("nginx.ingress.kubernetes.io/rewrite-target")
                        .cloned()
                })
                .and_then(|_rewrite_target| {
                    let hir = regex_syntax::parse(path_value).ok()?;
                    let got = regex_syntax::hir::literal::Extractor::new().extract(&hir);

                    let prefixes = got
                        .literals()?
                        .iter()
                        .map(|l| String::from_utf8_lossy(l.as_bytes()).to_string())
                        .collect::<Vec<_>>();

                    Some(TraefikMiddleware::with_prefix_strip(
                        format!("{name}-middleware"),
                        prefixes,
                    ))
                });

            match middleware {
                Some(middleware) => Ok(TraefikIngressRoute::with_rule_and_middlewares(
                    rule,
                    vec![middleware],
                )),
                None => Ok(TraefikIngressRoute::with_rule(rule)),
            }
        }
        _ => {
            // TODO warn that ingress class is unknown

            Ok(TraefikIngressRoute::with_rule(
                TraefikRouterRule::path_prefix_rule([path_value.clone()]),
            ))
        }
    }
}

pub fn convert_k8s_ingress_to_traefik_ingress(
    ingress: Ingress,
    services: &[V1Service],
) -> Result<(IngressRoute, Vec<Middleware>), Box<ConvertK8sIngressError>> {
    let Some(spec) = ingress.spec.as_ref() else {
        return Err(Box::new(ConvertK8sIngressError::NoSpec {
            ingress: K8sSource(ingress),
        }));
    };
    let Some(rules) = spec.rules.as_ref() else {
        return Err(Box::new(ConvertK8sIngressError::SpecWithoutRules {
            ingress: K8sSource(ingress),
        }));
    };

    let Some(path) = rules
        .iter()
        .filter_map(|rule| rule.http.as_ref())
        .find_map(|http| http.paths.first())
    else {
        return Err(Box::new(ConvertK8sIngressError::RuleWithoutHttpPaths {
            ingress: K8sSource(ingress),
        }));
    };

    let route = convert_k8s_ingress_to_traefik_rule(ingress.clone())?;

    let mut middlewares_refs = route
        .routes()
        .iter()
        .flat_map(|route| route.middlewares().iter())
        .filter_map(|middleware| {
            if middleware.is_strip_prefix() {
                return None;
            }

            Some(TraefikRuleMiddlewareRef {
                name: middleware.name.clone(),
                namespace: None,
            })
        })
        .collect::<Vec<_>>();

    middlewares_refs.extend(route.routes()[0].middlewares().iter().map(|m| {
        TraefikRuleMiddlewareRef {
            name: m.name.clone(),
            namespace: None,
        }
    }));

    let Some(service) = path.backend.service.as_ref() else {
        return Err(Box::new(ConvertK8sIngressError::NoBackendService {
            ingress: K8sSource(ingress),
        }));
    };

    let port = if let Some(port) = service
        .port
        .as_ref()
        .and_then(|port| port.number)
        .map(|p| p as u16)
    {
        port
    } else {
        let port_name = service.port.as_ref().and_then(|port| port.name.as_ref());

        let Some(service) = services
            .iter()
            .find(|s| s.metadata.name.as_ref() == Some(&service.name))
        else {
            return Err(Box::new(ConvertK8sIngressError::NoMatchingService {
                ingress: K8sSource(ingress),
            }));
        };

        let Some(port) = service
            .spec
            .as_ref()
            .and_then(|spec| spec.ports.as_ref())
            .and_then(|ports| {
                ports
                    .iter()
                    .filter(|port| port.name.as_ref() == port_name)
                    .map(|port| port.port)
                    .next()
            })
        else {
            return Err(Box::new(ConvertK8sIngressError::NoMatchingServicePort {
                ingress: K8sSource(ingress),
            }));
        };
        port as u16
    };

    let routes = vec![TraefikRuleSpec {
        kind: String::from("Rule"),
        r#match: route.routes()[0].rule().to_string(),
        middlewares: Some(middlewares_refs),
        services: vec![TraefikRuleService {
            kind: Some(String::from("Service")),
            name: service.name.clone(),
            port: Some(port),
        }],
    }];

    let middlewares = route
        .routes()
        .iter()
        .flat_map(|r| r.middlewares().iter())
        .map(|middleware| Middleware {
            metadata: kube::core::ObjectMeta {
                name: Some(middleware.name.clone()),
                ..Default::default()
            },
            spec: MiddlewareSpec(middleware.to_json_spec()),
        })
        .collect::<Vec<_>>();

    let route = IngressRoute {
        metadata: ObjectMeta {
            name: ingress.metadata.name,
            ..Default::default()
        },
        spec: IngressRouteSpec {
            routes: Some(routes),
            entry_points: Some(route.entry_points().clone()),
            tls: route.tls().as_ref().map(|tls| TraefikTls {
                cert_resolver: Some(tls.cert_resolver.clone()),
            }),
        },
    };

    Ok((route, middlewares))
}

/// Creates a JSON payload suitable for [Kubernetes'
/// Namespaces](https://kubernetes.io/docs/tasks/administer-cluster/namespaces/)
pub fn namespace_payload(
    app_name: &AppName,
    config: &Config,
    user_defined_parameters: &Option<UserDefinedParameters>,
    owners: &HashSet<Owner>,
) -> V1Namespace {
    V1Namespace {
        metadata: ObjectMeta {
            name: Some(app_name.to_rfc1123_namespace_id()),
            annotations: namespace_annotations(config, user_defined_parameters, owners),
            labels: Some(BTreeMap::from([(
                APP_NAME_LABEL.to_string(),
                app_name.to_string(),
            )])),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn namespace_annotations(
    config: &Config,
    user_defined_parameters: &Option<UserDefinedParameters>,
    owners: &HashSet<Owner>,
) -> Option<BTreeMap<String, String>> {
    let annotations = match &config.runtime {
        crate::config::Runtime::Docker => None,
        crate::config::Runtime::Kubernetes(runtime) => {
            let annotations = &runtime.annotations.namespace;

            if annotations.is_empty() {
                None
            } else {
                Some(annotations.clone())
            }
        }
    };

    let annotations = if let Some(user_defined_parameters) = user_defined_parameters {
        let mut annotations = annotations.unwrap_or_default();
        annotations.insert(
            USER_DEFINED_PARAMETERS_LABEL.to_string(),
            serde_json::to_string(user_defined_parameters).unwrap(),
        );
        Some(annotations)
    } else {
        annotations
    };

    if !owners.is_empty() {
        let mut annotations = annotations.unwrap_or_default();
        annotations.insert(
            OWNERS_LABEL.to_string(),
            serde_json::to_string(&owners).unwrap(),
        );
        Some(annotations)
    } else {
        annotations
    }
}

/// Creates a JSON payload suitable for [Kubernetes'
/// Deployments](https://kubernetes.io/docs/concepts/workloads/controllers/deployment/)
pub fn deployment_payload(
    app_name: &AppName,
    service: &DeployableService,
    container_config: &ContainerConfig,
    persistent_volume_map: &Option<HashMap<&String, PersistentVolumeClaim>>,
) -> V1Deployment {
    let env = service.blueprint_service.env.as_ref().map(|env| {
        env.iter()
            .map(|env| EnvVar {
                name: env.key().to_string(),
                value: Some(env.value().unsecure().to_string()),
                ..Default::default()
            })
            .collect()
    });

    let annotations = if let Some(replicated_env) = service
        .blueprint_service
        .env
        .as_ref()
        .and_then(super::super::replicated_environment_variable_to_json)
    {
        BTreeMap::from([
            (
                IMAGE_LABEL.to_string(),
                service.blueprint_service.image.to_string(),
            ),
            (REPLICATED_ENV_LABEL.to_string(), replicated_env.to_string()),
        ])
    } else {
        BTreeMap::from([(
            IMAGE_LABEL.to_string(),
            service.blueprint_service.image.to_string(),
        )])
    };

    let volume_mounts = service.blueprint_service.files.as_ref().map(|files| {
        let parent_paths = files
            .keys()
            .filter_map(|path| path.parent())
            .collect::<HashSet<_>>();

        parent_paths
            .iter()
            .map(|path| VolumeMount {
                name: secret_name_from_path!(path),
                mount_path: path.to_string_lossy().to_string(),
                ..Default::default()
            })
            .collect::<Vec<_>>()
    });

    let volume_mounts = match persistent_volume_map {
        Some(pv_map) => {
            let mut mounts = volume_mounts.unwrap_or_default();
            for (path, pvc) in pv_map {
                mounts.push(pvc_volume_mount_payload(path, pvc));
            }
            Some(mounts)
        }
        None => volume_mounts,
    };

    let volumes = service.blueprint_service.files.as_ref().map(|files| {
        let files = files
            .keys()
            .filter_map(|path| path.parent().map(|parent| (parent, path)))
            .collect::<MultiMap<_, _>>();

        files
            .iter_all()
            .map(|(parent, paths)| {
                let items = paths
                    .iter()
                    .map(|path| KeyToPath {
                        key: secret_name_from_name!(path),
                        path: path
                            .file_name()
                            .map_or(String::new(), |name| name.to_string_lossy().to_string()),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();

                Volume {
                    name: secret_name_from_path!(parent),
                    secret: Some(SecretVolumeSource {
                        secret_name: Some(format!(
                            "{}-{}-secret",
                            app_name, service.blueprint_service.service_name
                        )),
                        items: Some(items),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            })
            .collect::<Vec<Volume>>()
    });

    let volumes = match persistent_volume_map {
        Some(pv_map) => {
            let mut vols = volumes.unwrap_or_default();
            pv_map.iter().for_each(|(_, pvc)| {
                vols.push(pvc_volume_payload(pvc));
            });

            Some(vols)
        }
        None => volumes,
    };

    let resources = container_config
        .memory_limit()
        .map(|mem_limit| ResourceRequirements {
            limits: Some(BTreeMap::from([(
                String::from("memory"),
                Quantity(format!("{}", mem_limit.as_u64())),
            )])),
            ..Default::default()
        });

    // TODO service.labels() is not considered deprecated anymore and we should annotate the
    // deployment with lables.

    let labels = BTreeMap::from([
        (APP_NAME_LABEL.to_string(), app_name.to_string()),
        (
            SERVICE_NAME_LABEL.to_string(),
            service.blueprint_service.service_name.to_string(),
        ),
        (
            CONTAINER_TYPE_LABEL.to_string(),
            service.service_type.to_string(),
        ),
    ]);

    V1Deployment {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-{}-deployment",
                app_name.to_rfc1123_namespace_id(),
                service.blueprint_service.service_name
            )),
            namespace: Some(app_name.to_rfc1123_namespace_id()),
            labels: Some(labels.clone()),
            annotations: Some(annotations),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(1),
            selector: LabelSelector {
                match_labels: Some(labels.clone()),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    annotations: Some(deployment_annotations(&service.strategy)),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    volumes,
                    containers: vec![Container {
                        name: service.blueprint_service.service_name.to_string(),
                        image: Some(service.blueprint_service.image.to_string()),
                        image_pull_policy: Some(String::from("Always")),
                        env,
                        volume_mounts,
                        ports: Some(vec![ContainerPort {
                            container_port: service.port as i32,
                            ..Default::default()
                        }]),
                        resources,
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Creates the value of an [annotations object](https://kubernetes.io/docs/concepts/overview/working-with-objects/annotations/)
/// so that the underlying pod will be deployed according to its [deployment strategy](`DeploymentStrategy`).
///
/// For example, this [popular workaround](https://stackoverflow.com/a/55221174/5088458) will be
/// applied to ensure that a pod will be recreated everytime a deployment with
/// [`DeploymentStrategy::RedeployAlways`] has been initiated.
fn deployment_annotations(strategy: &DeploymentStrategy) -> BTreeMap<String, String> {
    match strategy {
        DeploymentStrategy::OnImageUpdate(image_id) => {
            BTreeMap::from([(String::from("imageHash"), image_id.clone())])
        }
        DeploymentStrategy::Never => BTreeMap::new(),
        DeploymentStrategy::Always => {
            BTreeMap::from([(String::from("date"), Utc::now().to_rfc3339())])
        }
    }
}

/// Creates a JSON payload suitable for [Kubernetes' Secrets](https://kubernetes.io/docs/concepts/configuration/secret/)
pub fn secrets_payload(app_name: &AppName, blueprint_service: &ServiceConfig) -> Option<V1Secret> {
    let secrets = blueprint_service
        .files
        .as_ref()?
        .iter()
        .map(|(path, file_content)| {
            (
                secret_name_from_name!(path),
                Value::String(general_purpose::STANDARD.encode(file_content.unsecure())),
            )
        })
        .collect::<Map<String, Value>>();

    serde_json::from_value(serde_json::json!({
      "apiVersion": "v1",
      "kind": "Secret",
      "metadata": {
        "name": format!("{}-{}-secret", app_name.to_rfc1123_namespace_id(), blueprint_service.service_name),
        "namespace": app_name.to_rfc1123_namespace_id(),
         APP_NAME_LABEL: app_name,
         SERVICE_NAME_LABEL: blueprint_service.service_name,
         // TODO: CONTAINER_TYPE_LABEL: blueprint_service.service_type.to_string()
      },
      "type": "Opaque",
      "data": secrets
    })).ok()
}

pub fn image_pull_secret_payload(
    app_name: &AppName,
    registries_and_credentials: BTreeMap<String, (&str, &SecUtf8)>,
) -> V1Secret {
    // Hashing over all registries ensures that the same secret name will be generated for the same
    // registries. Thus, password or user can change and will be updated. Additionally, it will be
    // idempontent to the Kubernetes API.
    let mut registry_hasher = DefaultHasher::new();
    for registry in registries_and_credentials.keys() {
        registry_hasher.write(registry.as_bytes());
    }

    let data = ByteString(
        serde_json::json!({
            "auths":
            serde_json::Map::from_iter(registries_and_credentials.into_iter().map(
                |(registry, (username, password))| {
                    (
                        registry,
                        serde_json::json!({
                            "username": username.to_string(),
                            "password": password.unsecure().to_string(),
                        }),
                    )
                },
            ))
        })
        .to_string()
        .into_bytes(),
    );

    V1Secret {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-image-pull-secret-{:#010x}",
                app_name.to_rfc1123_namespace_id(),
                registry_hasher.finish()
            )),
            namespace: Some(app_name.to_rfc1123_namespace_id()),
            labels: Some(BTreeMap::from([(
                APP_NAME_LABEL.to_string(),
                app_name.to_string(),
            )])),
            ..Default::default()
        },
        immutable: Some(true),
        data: Some(BTreeMap::from([(String::from(".dockerconfigjson"), data)])),
        type_: Some(String::from("kubernetes.io/dockerconfigjson")),
        ..Default::default()
    }
}

/// Creates a JSON payload suitable for [Kubernetes' Services](https://kubernetes.io/docs/concepts/services-networking/service/)
pub fn service_payload(app_name: &AppName, service_config: &DeployableService) -> V1Service {
    serde_json::from_value(serde_json::json!({
      "apiVersion": "v1",
      "kind": "Service",
      "namespace": app_name.to_rfc1123_namespace_id(),
      "metadata": {
        "name": service_config.blueprint_service.service_name,
        APP_NAME_LABEL: app_name,
        SERVICE_NAME_LABEL: service_config.blueprint_service.service_name,
        CONTAINER_TYPE_LABEL: service_config.service_type.to_string()
      },
      "spec": {
        "ports": [
          {
            "name": service_config.blueprint_service.service_name,
            "targetPort": service_config.port,
            "port": service_config.port
          }
        ],
        "selector": {
          APP_NAME_LABEL: app_name,
          SERVICE_NAME_LABEL: service_config.blueprint_service.service_name,
          CONTAINER_TYPE_LABEL: service_config.service_type.to_string()
        }
      }
    }))
    .expect("Cannot convert value to core/v1/Service")
}

pub fn convert_k8s_traefik_crds_to_domain_traefik_routes(
    routes: Vec<IngressRoute>,
    middlewares: Vec<Middleware>,
) -> Vec<(String, TraefikIngressRoute, Vec<TraefikRuleService>)> {
    let middlewares_by_name = middlewares
        .iter()
        .filter_map(|m| Some((m.metadata.name.as_deref()?, m)))
        .collect::<HashMap<_, _>>();

    let mut converted_routes = Vec::with_capacity(routes.len());
    for route in routes.into_iter() {
        let name = route.metadata.name.as_deref().unwrap_or_default();
        let entry_points = route.spec.entry_points.unwrap_or_default();
        let tls = route.spec.tls.and_then(|tls| tls.cert_resolver);

        for (i, route) in route
            .spec
            .routes
            .into_iter()
            .flat_map(|r| r.into_iter())
            .enumerate()
        {
            let rule = TraefikRouterRule::from_str(&route.r#match).unwrap();

            let middlewares = route
                .middlewares
                .iter()
                .flat_map(|m| m.iter())
                .filter_map(|m| Some((m.name.as_str(), middlewares_by_name.get(m.name.as_str())?)))
                .map(|(name, middleware)| {
                    TraefikMiddleware::from_json(name.to_string(), middleware.spec.0.clone())
                })
                .collect::<Vec<_>>();

            converted_routes.push((
                if i > 0 {
                    format!("{name}{i}")
                } else {
                    name.to_string()
                },
                TraefikIngressRoute::with_existing_routing_rules(
                    entry_points.clone(),
                    rule,
                    middlewares,
                    tls.clone(),
                ),
                route.services,
            ));
        }
    }
    converted_routes
}

pub fn ingress_route_payload_base(app_name: &AppName, route: &TraefikIngressRoute) -> IngressRoute {
    let rules = route
        .routes()
        .iter()
        .map(|route| {
            let middlewares = route
                .middlewares()
                .iter()
                .map(|middleware| TraefikRuleMiddlewareRef {
                    name: AppName::from_str(middleware.name.as_str())
                        .unwrap()
                        .to_rfc1123_namespace_id(),
                    namespace: None,
                })
                .collect::<Vec<_>>();

            TraefikRuleSpec {
                kind: String::from("Rule"),
                r#match: route.rule().to_string(),
                middlewares: Some(middlewares),
                services: Vec::new(),
            }
        })
        .collect::<Vec<_>>();

    IngressRoute {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-ingress-route",
                app_name.to_rfc1123_namespace_id(),
            )),
            namespace: Some(app_name.to_rfc1123_namespace_id()),
            annotations: Some(BTreeMap::from([
                (APP_NAME_LABEL.to_string(), app_name.to_string()),
                (
                    String::from("traefik.ingress.kubernetes.io/router.entrypoints"),
                    String::from("web"),
                ),
            ])),
            ..Default::default()
        },
        spec: IngressRouteSpec {
            routes: Some(rules),
            entry_points: Some(route.entry_points().clone()),
            tls: route.tls().as_ref().map(|tls| TraefikTls {
                cert_resolver: Some(tls.cert_resolver.clone()),
            }),
        },
    }
}

/// Creates a payload that ensures that Traefik find the correct route in Kubernetes
///
/// See [Traefik Routers](https://docs.traefik.io/v2.0/user-guides/crd-acme/#traefik-routers)
/// for more information.
pub fn ingress_route_payload(
    app_name: &AppName,
    blueprint_service: &ServiceConfig,
    route: &TraefikIngressRoute,
    service_type: &ContainerType,
    port: Option<u16>,
) -> IngressRoute {
    let rules = route
        .routes()
        .iter()
        .map(|route| {
            let middlewares = route
                .middlewares()
                .iter()
                .map(|middleware| TraefikRuleMiddlewareRef {
                    name: AppName::from_str(middleware.name.as_str())
                        .unwrap()
                        .to_rfc1123_namespace_id(),
                    namespace: None,
                })
                .collect::<Vec<_>>();

            TraefikRuleSpec {
                kind: String::from("Rule"),
                r#match: route.rule().to_string(),
                middlewares: Some(middlewares),
                services: vec![TraefikRuleService {
                    kind: Some(String::from("Service")),
                    name: blueprint_service.service_name.to_string(),
                    port,
                }],
            }
        })
        .collect::<Vec<_>>();

    IngressRoute {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-{}-ingress-route",
                app_name.to_rfc1123_namespace_id(),
                blueprint_service.service_name
            )),
            namespace: Some(app_name.to_rfc1123_namespace_id()),
            annotations: Some(BTreeMap::from([
                (APP_NAME_LABEL.to_string(), app_name.to_string()),
                (
                    SERVICE_NAME_LABEL.to_string(),
                    blueprint_service.service_name.to_string(),
                ),
                (CONTAINER_TYPE_LABEL.to_string(), service_type.to_string()),
                (
                    String::from("traefik.ingress.kubernetes.io/router.entrypoints"),
                    String::from("web"),
                ),
            ])),
            ..Default::default()
        },
        spec: IngressRouteSpec {
            routes: Some(rules),
            entry_points: Some(route.entry_points().clone()),
            tls: route.tls().as_ref().map(|tls| TraefikTls {
                cert_resolver: Some(tls.cert_resolver.clone()),
            }),
        },
    }
}

/// See [Traefik Routers](https://docs.traefik.io/v2.0/user-guides/crd-acme/#traefik-routers)
/// for more information.
pub fn middleware_payload(
    app_name: &AppName,
    ingress_route: &TraefikIngressRoute,
) -> Vec<Middleware> {
    ingress_route
        .routes()
        .iter()
        .flat_map(|r| {
            r.middlewares().iter().filter_map(|middleware| {
                Some((
                    AppName::from_str(middleware.name.as_str())
                        .ok()?
                        .to_rfc1123_namespace_id(),
                    middleware.to_json_spec(),
                ))
            })
        })
        .map(|(name, spec)| Middleware {
            metadata: ObjectMeta {
                name: Some(name.clone()),
                namespace: Some(app_name.to_rfc1123_namespace_id()),
                ..Default::default()
            },
            spec: MiddlewareSpec(serde_json::json!(spec)),
        })
        .collect::<Vec<_>>()
}

pub fn pvc_volume_mount_payload(
    path: &str,
    persitent_volume_claim: &PersistentVolumeClaim,
) -> VolumeMount {
    VolumeMount {
        name: format!(
            "{}-volume",
            persitent_volume_claim
                .metadata
                .labels
                .as_ref()
                .unwrap_or(&BTreeMap::new())
                .get(STORAGE_TYPE_LABEL)
                .unwrap_or(&String::from("default"))
        ),
        mount_path: path.to_string(),
        ..Default::default()
    }
}

pub fn pvc_volume_payload(persistent_volume_claim: &PersistentVolumeClaim) -> Volume {
    Volume {
        name: format!(
            "{}-volume",
            persistent_volume_claim
                .metadata
                .labels
                .as_ref()
                .unwrap_or(&BTreeMap::new())
                .get(STORAGE_TYPE_LABEL)
                .unwrap_or(&String::from("default"))
        ),
        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
            claim_name: persistent_volume_claim
                .metadata
                .name
                .clone()
                .unwrap_or_default(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub fn persistent_volume_claim_payload(
    app_name: &AppName,
    service: &DeployableService,
    storage_size: &ByteSize,
    storage_class: &str,
    declared_volume: &str,
) -> PersistentVolumeClaim {
    PersistentVolumeClaim {
        metadata: ObjectMeta {
            generate_name: Some(format!(
                "{}-{}-pvc-",
                app_name.to_rfc1123_namespace_id(),
                service.blueprint_service.service_name
            )),
            labels: Some(BTreeMap::from([
                (APP_NAME_LABEL.to_owned(), app_name.to_string()),
                (
                    SERVICE_NAME_LABEL.to_owned(),
                    service.blueprint_service.service_name.clone(),
                ),
                (
                    STORAGE_TYPE_LABEL.to_owned(),
                    declared_volume
                        .split('/')
                        .next_back()
                        .unwrap_or("default")
                        .to_owned(),
                ),
            ])),
            ..Default::default()
        },
        spec: Some(PersistentVolumeClaimSpec {
            storage_class_name: Some(storage_class.to_owned()),
            access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
            resources: Some(VolumeResourceRequirements {
                requests: Some(BTreeMap::from_iter(vec![(
                    "storage".to_owned(),
                    Quantity(format!("{}", storage_size.as_u64())),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        app_blueprints::EnvironmentVariable, app_deployment::AppDeploymentBuilder,
        blueprint_service,
    };
    use std::str::FromStr;

    macro_rules! deployment_object {
        ($deployment_name:expr_2021, $app_name:expr_2021, $service_name:expr_2021, $image:expr_2021, $container_type:expr_2021, $($a_key:expr_2021 => $a_value:expr_2021),*) => {{
            let mut labels = BTreeMap::new();

            if let Some(app_name) = $app_name {
                labels.insert(String::from(APP_NAME_LABEL), app_name);
            }
            if let Some(service_name) = $service_name {
                labels.insert(String::from(SERVICE_NAME_LABEL), service_name);
            }
            if let Some(container_type) = $container_type {
                labels.insert(String::from(CONTAINER_TYPE_LABEL), container_type);
            }

            let mut annotations = BTreeMap::new();
            if let Some(image) = $image {
                annotations.insert(String::from(IMAGE_LABEL), image);
            }

            $( annotations.insert(String::from($a_key), $a_value); )*

            V1Deployment {
                metadata: ObjectMeta {
                    name: Some(String::from($deployment_name)),
                    labels: Some(labels),
                    annotations: Some(annotations),
                    ..Default::default()
                },
                spec: Some(DeploymentSpec::default()),
                ..Default::default()
            }
        }};
    }

    #[test]
    fn should_create_deployment_payload() {
        let config = blueprint_service!("db", "mariadb:10.3.17");

        let deployment_unit = AppDeploymentBuilder::init(AppName::master(), vec![config], None)
            .finish()
            .unwrap();

        let payload = deployment_payload(
            &deployment_unit.app_name,
            &deployment_unit.services[0],
            &ContainerConfig::default(),
            &None,
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
              "apiVersion": "apps/v1",
              "kind": "Deployment",
              "metadata": {
                "annotations": {
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17"
                },
                "labels": {
                  "com.aixigo.preview.servant.app-name": "master",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db"
                },
                "name": "master-db-deployment",
                "namespace": "master"
              },
              "spec": {
                "replicas": 1,
                "selector": {
                  "matchLabels": {
                    "com.aixigo.preview.servant.app-name": "master",
                    "com.aixigo.preview.servant.container-type": "instance",
                    "com.aixigo.preview.servant.service-name": "db"
                  }
                },
                "template": {
                  "metadata": {
                    "annotations": {
                    },
                    "labels": {
                      "com.aixigo.preview.servant.app-name": "master",
                      "com.aixigo.preview.servant.container-type": "instance",
                      "com.aixigo.preview.servant.service-name": "db"
                    }
                  },
                  "spec": {
                    "containers": [
                      {
                        "image": "docker.io/library/mariadb:10.3.17",
                        "imagePullPolicy": "Always",
                        "name": "db",
                        "ports": [
                          {
                            "containerPort": 80
                          }
                        ]
                      }
                    ]
                  }
                }
              }
            })
        );
    }

    #[test]
    fn should_create_deployment_with_environment_variable() {
        let config = blueprint_service!("db", "mariadb:10.3.17", env = (
                "MYSQL_ROOT_PASSWORD" => "example"
        ));

        let deployment_unit = AppDeploymentBuilder::init(AppName::master(), vec![config], None)
            .finish()
            .unwrap();

        let payload = deployment_payload(
            &deployment_unit.app_name,
            &deployment_unit.services[0],
            &ContainerConfig::default(),
            &None,
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
              "apiVersion": "apps/v1",
              "kind": "Deployment",
              "metadata": {
                "annotations": {
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
                },
                "labels": {
                  "com.aixigo.preview.servant.app-name": "master",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db"
                },
                "name": "master-db-deployment",
                "namespace": "master"
              },
              "spec": {
                "replicas": 1,
                "selector": {
                  "matchLabels": {
                    "com.aixigo.preview.servant.app-name": "master",
                    "com.aixigo.preview.servant.container-type": "instance",
                    "com.aixigo.preview.servant.service-name": "db"
                  }
                },
                "template": {
                  "metadata": {
                    "annotations": {
                    },
                    "labels": {
                      "com.aixigo.preview.servant.app-name": "master",
                      "com.aixigo.preview.servant.container-type": "instance",
                      "com.aixigo.preview.servant.service-name": "db"
                    }
                  },
                  "spec": {
                    "containers": [
                      {
                        "env": [],
                        "image": "docker.io/library/mariadb:10.3.17",
                        "imagePullPolicy": "Always",
                        "name": "db",
                        "ports": [
                          {
                            "containerPort": 80
                          }
                        ],
                      }
                    ],
                  }
                }
              }
            })
        );
    }

    #[test]
    fn should_create_deployment_with_replicated_environment_variable() {
        let mut config = blueprint_service!("db", "mariadb:10.3.17");
        config.add_env(EnvironmentVariable::with_replicated(
            String::from("MYSQL_ROOT_PASSWORD"),
            SecUtf8::from("example"),
        ));

        let deployment_unit = AppDeploymentBuilder::init(AppName::master(), vec![config], None)
            .finish()
            .unwrap();

        let payload = deployment_payload(
            &deployment_unit.app_name,
            &deployment_unit.services[0],
            &ContainerConfig::default(),
            &None,
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
              "apiVersion": "apps/v1",
              "kind": "Deployment",
              "metadata": {
                "annotations": {
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
                  "com.aixigo.preview.servant.replicated-env": serde_json::json!({
                      "MYSQL_ROOT_PASSWORD": {
                        "value": "example",
                        "templated": false,
                        "replicate": true,
                      }
                    }).to_string()
                },
                "labels": {
                  "com.aixigo.preview.servant.app-name": "master",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db"
                },
                "name": "master-db-deployment",
                "namespace": "master"
              },
              "spec": {
                "replicas": 1,
                "selector": {
                  "matchLabels": {
                    "com.aixigo.preview.servant.app-name": "master",
                    "com.aixigo.preview.servant.container-type": "instance",
                    "com.aixigo.preview.servant.service-name": "db"
                  }
                },
                "template": {
                  "metadata": {
                    "annotations": {
                    },
                    "labels": {
                      "com.aixigo.preview.servant.app-name": "master",
                      "com.aixigo.preview.servant.container-type": "instance",
                      "com.aixigo.preview.servant.service-name": "db"
                    }
                  },
                  "spec": {
                    "containers": [
                      {
                        "env": [],
                        "image": "docker.io/library/mariadb:10.3.17",
                        "imagePullPolicy": "Always",
                        "name": "db",
                        "ports": [
                          {
                            "containerPort": 80
                          }
                        ]
                      }
                    ]
                  }
                }
              }
            })
        );
    }

    #[test]
    fn should_create_deployment_payload_with_app_name_that_is_not_compliant_to_rfc1123() {
        let config = blueprint_service!("db", "mariadb:10.3.17");

        let deployment_unit =
            AppDeploymentBuilder::init(AppName::from_str("MY-APP").unwrap(), vec![config], None)
                .finish()
                .unwrap();

        let payload = deployment_payload(
            &deployment_unit.app_name,
            &deployment_unit.services[0],
            &ContainerConfig::default(),
            &None,
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
              "apiVersion": "apps/v1",
              "kind": "Deployment",
              "metadata": {
                "annotations": {
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17"
                },
                "labels": {
                  "com.aixigo.preview.servant.app-name": "MY-APP",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db"
                },
                "name": "my-app-db-deployment",
                "namespace": "my-app"
              },
              "spec": {
                "replicas": 1,
                "selector": {
                  "matchLabels": {
                    "com.aixigo.preview.servant.app-name": "MY-APP",
                    "com.aixigo.preview.servant.container-type": "instance",
                    "com.aixigo.preview.servant.service-name": "db"
                  }
                },
                "template": {
                  "metadata": {
                    "annotations": {
                    },
                    "labels": {
                      "com.aixigo.preview.servant.app-name": "MY-APP",
                      "com.aixigo.preview.servant.container-type": "instance",
                      "com.aixigo.preview.servant.service-name": "db"
                    }
                  },
                  "spec": {
                    "containers": [
                      {
                        "image": "docker.io/library/mariadb:10.3.17",
                        "imagePullPolicy": "Always",
                        "name": "db",
                        "ports": [
                          {
                            "containerPort": 80
                          }
                        ]
                      }
                    ]
                  }
                }
              }
            })
        );
    }

    #[test]
    fn create_traefik_crd_ingress_route() {
        let config = blueprint_service!("db", "mariadb:10.3.17");

        let mut deployment_unit = AppDeploymentBuilder::init(AppName::master(), vec![config], None)
            .finish()
            .unwrap();

        let service = &mut deployment_unit.services[0];
        service.port = 1234;

        let payload = ingress_route_payload(
            &deployment_unit.app_name,
            &service.blueprint_service,
            &service.ingress_route,
            &service.service_type,
            Some(service.port),
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
              "apiVersion": "traefik.containo.us/v1alpha1",
              "kind": "IngressRoute",
              "metadata": {
                "name": "master-db-ingress-route",
                "namespace": "master",
              },
              "spec": {
                "routes": [
                  {
                    "match": "PathPrefix(`/master/db/`)",
                    "kind": "Rule",
                    "services": [
                      {
                        "name": "db",
                        "port": 1234,
                      }
                    ],
                    "middlewares": [
                      {
                        "name": "master-db-middleware",
                      }
                    ]
                  }
                ]
              },
            }),
        );
    }

    #[test]
    fn should_create_ingress_route_with_app_name_that_is_not_compliant_to_rfc1123() {
        let config = blueprint_service!("db", "mariadb:10.3.17");

        let mut deployment_unit =
            AppDeploymentBuilder::init(AppName::from_str("MY-APP").unwrap(), vec![config], None)
                .finish()
                .unwrap();

        let service = &mut deployment_unit.services[0];
        service.port = 1234;

        let payload = ingress_route_payload(
            &deployment_unit.app_name,
            &service.blueprint_service,
            &service.ingress_route,
            &service.service_type,
            Some(service.port),
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
              "apiVersion": "traefik.containo.us/v1alpha1",
              "kind": "IngressRoute",
              "metadata": {
                "name": "my-app-db-ingress-route",
                "namespace": "my-app",
              },
              "spec": {
                "routes": [
                  {
                    "match": "PathPrefix(`/MY-APP/db/`)",
                    "kind": "Rule",
                    "services": [
                      {
                        "name": "db",
                        "port": 1234,
                      }
                    ],
                    "middlewares": [
                      {
                        "name": "my-app-db-middleware",
                      }
                    ]
                  }
                ]
              },
            }),
        );
    }

    #[test]
    fn should_create_middleware_with_default_prefix() {
        let app_name = AppName::master();

        let payload = middleware_payload(
            &app_name,
            &TraefikIngressRoute::with_defaults(&app_name, "db"),
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!([{
              "apiVersion": "traefik.containo.us/v1alpha1",
              "kind": "Middleware",
              "metadata": {
                "name": "master-db-middleware",
                "namespace": "master",
              },
              "spec": {
                "stripPrefix": {
                  "prefixes": [
                    "/master/db/"
                  ]
                }
              },
            }]),
        );
    }

    #[rstest::rstest]
    #[case(TraefikIngressRoute::with_app_only_defaults(&AppName::master()))]
    #[case(TraefikIngressRoute::with_defaults(&AppName::master(), "nextcloud"))]
    #[case(TraefikIngressRoute::with_existing_routing_rules(
            vec![String::from("websecure")],
            TraefikRouterRule::from_str("Host(`example.com`) && PathPrefix(`/test`)").unwrap(),
            vec![TraefikMiddleware::with_forward_auth(
                String::from("auth"),
                url::Url::from_str("https://auth.example.com").unwrap()
            )],
            Some(String::from("tls")),
    ))]
    fn convert_back_traefik_domain(#[case] route: TraefikIngressRoute) {
        let ingress = ingress_route_payload_base(&AppName::master(), &route);
        let middlewares = middleware_payload(&AppName::master(), &route);

        let routes = convert_k8s_traefik_crds_to_domain_traefik_routes(vec![ingress], middlewares);

        assert_eq!(routes, vec![(String::from("master-ingress-route"), route, Vec::new())]);
    }

    #[test]
    fn should_create_middleware_with_default_prefix_with_name_rfc1123_app_name() {
        let app_name = AppName::from_str("MY-APP").unwrap();

        let payload = middleware_payload(
            &app_name,
            &TraefikIngressRoute::with_defaults(&app_name, "db"),
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!([{
              "apiVersion": "traefik.containo.us/v1alpha1",
              "kind": "Middleware",
              "metadata": {
                "name": "my-app-db-middleware",
                "namespace": "my-app",
              },
              "spec": {
                "stripPrefix": {
                  "prefixes": [
                    "/MY-APP/db/"
                  ]
                }
              },
            }]),
        );
    }

    #[test]
    fn should_create_deployment_payload_with_persistent_volume_claim() {
        let config = blueprint_service!("db", "mariadb:10.3.17");

        let mut deployment_unit = AppDeploymentBuilder::init(AppName::master(), vec![config], None)
            .finish()
            .unwrap();
        deployment_unit.services[0]
            .declared_volumes
            .push(String::from("/var/lib/data"));

        let payload = deployment_payload(
            &deployment_unit.app_name,
            &deployment_unit.services[0],
            &ContainerConfig::default(),
            &Some(HashMap::from([(
                &String::from("/var/lib/data"),
                PersistentVolumeClaim {
                    metadata: ObjectMeta {
                        name: Some(String::from("master-db-pvc-abc")),
                        namespace: Some(String::from("master")),
                        labels: Some(BTreeMap::from([
                            (APP_NAME_LABEL.to_owned(), "master".to_owned()),
                            (SERVICE_NAME_LABEL.to_owned(), "db".to_owned()),
                            (STORAGE_TYPE_LABEL.to_owned(), "data".to_owned()),
                        ])),
                        ..Default::default()
                    },
                    spec: Some(PersistentVolumeClaimSpec {
                        storage_class_name: Some("local-path".to_owned()),
                        access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
                        resources: Some(VolumeResourceRequirements {
                            requests: Some(BTreeMap::from_iter(vec![(
                                "storage".to_owned(),
                                Quantity("2Gi".to_owned()),
                            )])),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )])),
        );

        assert_json_diff::assert_json_include!(
            actual:payload,
            expected:serde_json::json!({
              "apiVersion": "apps/v1",
              "kind": "Deployment",
              "metadata": {
                "annotations": {
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17"
                },
                "labels": {
                  "com.aixigo.preview.servant.app-name": "master",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db"
                },
                "name": "master-db-deployment",
                "namespace": "master"
              },
              "spec": {
                "replicas": 1,
                "selector": {
                  "matchLabels": {
                    "com.aixigo.preview.servant.app-name": "master",
                    "com.aixigo.preview.servant.container-type": "instance",
                    "com.aixigo.preview.servant.service-name": "db"
                  }
                },
                "template": {
                  "metadata": {
                    "annotations": {
                    },
                    "labels": {
                      "com.aixigo.preview.servant.app-name": "master",
                      "com.aixigo.preview.servant.container-type": "instance",
                      "com.aixigo.preview.servant.service-name": "db"
                    }
                  },
                  "spec": {
                    "containers": [
                      {
                        "image": "docker.io/library/mariadb:10.3.17",
                        "imagePullPolicy": "Always",
                        "name": "db",
                        "ports": [
                          {
                            "containerPort": 80
                          }
                        ],
                        "volumeMounts": [{
                          "mountPath": "/var/lib/data",
                          "name": "data-volume"
                        }]
                      }
                    ],
                    "volumes": [
                      {
                        "name": "data-volume",
                        "persistentVolumeClaim": {
                          "claimName": "master-db-pvc-abc"
                        }
                      }
                    ]
                  }
                }
              }
            })
        );
    }

    #[test]
    fn should_create_deployment_for_config_containing_file_data() {
        let config = blueprint_service!("db", "mariadb:10.3.17", env = (), files = (
            "/etc/mysql/my.cnf" =>
                r"[client-server]
                  socket=/tmp/mysql.sock
                  port=3306"
        ));

        let deployment_unit = AppDeploymentBuilder::init(AppName::master(), vec![config], None)
            .finish()
            .unwrap();

        let payload = deployment_payload(
            &deployment_unit.app_name,
            &deployment_unit.services[0],
            &ContainerConfig::default(),
            &None,
        );

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
              "apiVersion": "apps/v1",
              "kind": "Deployment",
              "metadata": {
                "annotations": {
                  "com.aixigo.preview.servant.image": "docker.io/library/mariadb:10.3.17",
                },
                "labels": {
                  "com.aixigo.preview.servant.app-name": "master",
                  "com.aixigo.preview.servant.container-type": "instance",
                  "com.aixigo.preview.servant.service-name": "db"
                },
                "name": "master-db-deployment",
                "namespace": "master"
              },
              "spec": {
                "replicas": 1,
                "selector": {
                  "matchLabels": {
                    "com.aixigo.preview.servant.app-name": "master",
                    "com.aixigo.preview.servant.container-type": "instance",
                    "com.aixigo.preview.servant.service-name": "db"
                  }
                },
                "template": {
                  "metadata": {
                    "annotations": {
                    },
                    "labels": {
                      "com.aixigo.preview.servant.app-name": "master",
                      "com.aixigo.preview.servant.container-type": "instance",
                      "com.aixigo.preview.servant.service-name": "db"
                    }
                  },
                  "spec": {
                    "containers": [
                      {
                        "image": "docker.io/library/mariadb:10.3.17",
                        "imagePullPolicy": "Always",
                        "name": "db",
                        "ports": [
                          {
                            "containerPort": 80
                          }
                        ],
                        "volumeMounts": [{
                          "mountPath": "/etc/mysql",
                          "name": "etc-mysql"
                        }]
                      }
                    ],
                    "volumes": [{
                      "name": "etc-mysql",
                      "secret": {
                        "items": [
                          {
                            "key": "my-cnf",
                            "path": "my.cnf"
                          }
                        ],
                        "secretName": "master-db-secret"
                      }
                    }]
                  },
                }
              }
            })
        );
    }

    #[test]
    fn create_namespace_with_screaming_snake_case() {
        let namespace = namespace_payload(
            &AppName::from_str("MY-APP").unwrap(),
            &Default::default(),
            &None,
            &HashSet::new(),
        );

        assert_eq!(
            namespace,
            V1Namespace {
                metadata: ObjectMeta {
                    name: Some(String::from("my-app")),
                    labels: Some(BTreeMap::from([(
                        String::from("com.aixigo.preview.servant.app-name"),
                        String::from("MY-APP"),
                    )])),
                    ..Default::default()
                },
                ..Default::default()
            }
        );
    }

    #[test]
    fn create_namespace_payload_with_annotations() {
        let config = toml::de::from_str::<Config>(
            r#"
            [runtime]
            type = 'Kubernetes'
            [runtime.annotations.namespace]
            'field.cattle.io/projectId' = 'rancher-project-id'
            "#,
        )
        .unwrap();

        let namespace = namespace_payload(
            &AppName::from_str("myapp").unwrap(),
            &config,
            &None,
            &HashSet::new(),
        );

        assert_eq!(
            namespace,
            V1Namespace {
                metadata: ObjectMeta {
                    name: Some(String::from("myapp")),
                    labels: Some(BTreeMap::from([(
                        String::from("com.aixigo.preview.servant.app-name"),
                        String::from("myapp"),
                    )])),
                    annotations: Some(BTreeMap::from([(
                        String::from("field.cattle.io/projectId"),
                        String::from("rancher-project-id"),
                    )])),
                    ..Default::default()
                },
                ..Default::default()
            }
        );
    }

    #[test]
    fn create_image_pull_secrets() {
        let payload = image_pull_secret_payload(
            &AppName::from_str("MY-APP").unwrap(),
            BTreeMap::from([(
                String::from("registry.gitlab.com"),
                ("oauth2", &SecUtf8::from_str("some-random-token").unwrap()),
            )]),
        );

        assert_eq!(
            payload,
            V1Secret {
                metadata: ObjectMeta {
                    name: Some(String::from("my-app-image-pull-secret-0x7a2952c7a89d3fd0")),
                    namespace: Some(String::from("my-app")),
                    labels: Some(BTreeMap::from([(
                        String::from("com.aixigo.preview.servant.app-name"),
                        String::from("MY-APP")
                    )])),
                    ..Default::default()
                },
                immutable: Some(true),
                data: Some(BTreeMap::from([(
                    String::from(".dockerconfigjson"),
                    ByteString(
                        serde_json::json!({
                            "auths": {
                                "registry.gitlab.com": {
                                    "username": "oauth2",
                                    "password": "some-random-token"
                                }
                            }
                        })
                        .to_string()
                        .into_bytes()
                    )
                )])),
                type_: Some(String::from("kubernetes.io/dockerconfigjson")),
                ..Default::default()
            }
        )
    }

    #[test]
    fn should_parse_service_from_deployment_spec() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            None,
        );

        let service = kubernetes_object_to_service(deployment, None).unwrap();

        assert_eq!(service.service_name(), &String::from("nginx"));
    }

    #[test]
    fn should_parse_service_from_deployment_spec_with_replicated_env() {
        let deployment = deployment_object!(
            "master-db",
            Some(String::from("master")),
            Some(String::from("db")),
            Some(String::from("mariadb")),
            None,
            REPLICATED_ENV_LABEL => serde_json::json!({ "MYSQL_ROOT_PASSWORD": { "value": "example" } }).to_string()
        );

        let service = kubernetes_object_to_service(deployment, None).unwrap();

        assert_eq!(
            service.blueprint_config.env.unwrap().iter().next().unwrap(),
            &EnvironmentVariable::with_replicated(
                String::from("MYSQL_ROOT_PASSWORD"),
                SecUtf8::from("example")
            )
        );
    }

    #[test]
    fn should_parse_service_from_deployment_spec_without_container_type() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            None,
        );

        let service = kubernetes_object_to_service(deployment, None).unwrap();

        assert_eq!(service.service_type, ContainerType::Instance);
    }

    #[test]
    fn should_parse_service_from_deployment_spec_with_container_type() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            Some(String::from("replica")),
        );

        let service = kubernetes_object_to_service(deployment, None).unwrap();

        assert_eq!(service.service_type, ContainerType::Replica);
    }

    #[test]
    fn should_parse_service_from_deployment_spec_with_missing_service_name_label() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            None,
            Some(String::from("nginx")),
            None,
        );

        let service = kubernetes_object_to_service(deployment, None).unwrap();
        assert_eq!(service.service_name(), "master-nginx");
    }

    #[test]
    fn should_not_parse_service_from_deployment_spec_invalid_container_type() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            Some(String::from("nginx")),
            Some(String::from("abc")),
        );

        let err = kubernetes_object_to_service(deployment, None).unwrap_err();
        assert!(
            matches!(err, KubernetesInfrastructureError::UnknownServiceType {
                    unknown_label
                } if unknown_label == "abc"
            )
        );
    }

    #[test]
    fn should_not_parse_service_from_deployment_spec_due_to_missing_image_name() {
        let deployment = deployment_object!(
            "master-nginx",
            Some(String::from("master")),
            Some(String::from("nginx")),
            None,
            None,
        );

        let err = kubernetes_object_to_service(deployment, None).unwrap_err();
        assert!(matches!(err,
            KubernetesInfrastructureError::MissingImageLabel {
                deployment_name
            } if deployment_name == "master-nginx"
        ));
    }

    mod convert_k8s_ingress_to_traefik_ingress {
        use super::super::*;
        use assert_json_diff::assert_json_include;
        use k8s_openapi::api::{core::v1::ServicePort, networking::v1::*};
        use pretty_assertions::assert_eq;

        #[test]
        fn nginx_rewrite_without_path_type() -> Result<(), Box<ConvertK8sIngressError>> {
            let (route, middlewares) = convert_k8s_ingress_to_traefik_ingress(
                Ingress {
                    metadata: ObjectMeta {
                        name: Some(String::from("my-ingress")),
                        annotations: Some(BTreeMap::from([
                            (
                                String::from("nginx.ingress.kubernetes.io/use-regex"),
                                String::from("true"),
                            ),
                            (
                                String::from("nginx.ingress.kubernetes.io/rewrite-target"),
                                String::from("/$2"),
                            ),
                        ])),
                        ..Default::default()
                    },
                    spec: Some(IngressSpec {
                        ingress_class_name: Some(String::from("nginx")),
                        rules: Some(vec![IngressRule {
                            http: Some(HTTPIngressRuleValue {
                                paths: vec![HTTPIngressPath {
                                    path: Some(String::from("/my-service/")),
                                    backend: IngressBackend {
                                        service: Some(IngressServiceBackend {
                                            name: String::from("backend-service"),
                                            port: Some(ServiceBackendPort {
                                                number: Some(8080),
                                                ..Default::default()
                                            }),
                                        }),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }],
                            }),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                &[],
            )?;

            assert_eq!(
                route,
                IngressRoute {
                    metadata: ObjectMeta {
                        name: Some(String::from("my-ingress")),
                        ..Default::default()
                    },
                    spec: IngressRouteSpec {
                        entry_points: Some(vec![]),
                        routes: Some(vec![TraefikRuleSpec {
                            kind: String::from("Rule"),
                            r#match: String::from("PathPrefix(`/`)"),
                            services: vec![TraefikRuleService {
                                kind: Some(String::from("Service")),
                                name: String::from("backend-service"),
                                port: Some(8080)
                            }],
                            middlewares: Some(vec![TraefikRuleMiddlewareRef {
                                name: String::from("my-ingress-middleware"),
                                namespace: None
                            }])
                        }]),
                        tls: None
                    }
                }
            );

            assert_eq!(
                middlewares,
                vec![Middleware {
                    metadata: ObjectMeta {
                        name: Some(String::from("my-ingress-middleware")),
                        ..Default::default()
                    },
                    spec: MiddlewareSpec(serde_json::json!({
                        "stripPrefix": {
                            "prefixes": [
                                "/my-service/"
                            ]
                        }
                    }))
                }]
            );

            Ok(())
        }

        #[test]
        fn nginx_rewrite_with_path_type() {
            let (route, middlewares) = super::convert_k8s_ingress_to_traefik_ingress(
                Ingress {
                    metadata: ObjectMeta {
                        name: Some(String::from("my-ingress")),
                        annotations: Some(BTreeMap::from([
                            (
                                String::from("nginx.ingress.kubernetes.io/use-regex"),
                                String::from("true"),
                            ),
                            (
                                String::from("nginx.ingress.kubernetes.io/rewrite-target"),
                                String::from("/$2"),
                            ),
                        ])),
                        ..Default::default()
                    },
                    spec: Some(IngressSpec {
                        ingress_class_name: Some(String::from("nginx")),
                        rules: Some(vec![IngressRule {
                            http: Some(HTTPIngressRuleValue {
                                paths: vec![HTTPIngressPath {
                                    path: Some(String::from("/my-service/")),
                                    path_type: String::from("Prefix"),
                                    backend: IngressBackend {
                                        service: Some(IngressServiceBackend {
                                            name: String::from("backend-service"),
                                            port: Some(ServiceBackendPort {
                                                number: Some(8080),
                                                ..Default::default()
                                            }),
                                        }),
                                        ..Default::default()
                                    },
                                }],
                            }),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                &[],
            )
            .unwrap();

            assert_eq!(
                route,
                IngressRoute {
                    metadata: ObjectMeta {
                        name: Some(String::from("my-ingress")),
                        ..Default::default()
                    },
                    spec: IngressRouteSpec {
                        entry_points: Some(vec![]),
                        routes: Some(vec![TraefikRuleSpec {
                            kind: String::from("Rule"),
                            r#match: String::from("PathPrefix(`/my-service/`)"),
                            services: vec![TraefikRuleService {
                                kind: Some(String::from("Service")),
                                name: String::from("backend-service"),
                                port: Some(8080)
                            }],
                            middlewares: Some(vec![TraefikRuleMiddlewareRef {
                                name: String::from("my-ingress-middleware"),
                                namespace: None
                            }])
                        }]),
                        tls: None
                    }
                }
            );

            assert_eq!(
                middlewares,
                vec![Middleware {
                    metadata: ObjectMeta {
                        name: Some(String::from("my-ingress-middleware")),
                        ..Default::default()
                    },
                    spec: MiddlewareSpec(serde_json::json!({
                        "stripPrefix": {
                            "prefixes": [
                                "/my-service/"
                            ]
                        }
                    }))
                }]
            );
        }

        #[test]
        fn convert_k8s_ingress_to_traefik_ingress_with_missing_port() {
            let (route, _middlewares) = super::convert_k8s_ingress_to_traefik_ingress(
                Ingress {
                    metadata: ObjectMeta {
                        name: Some(String::from("schema-registry")),
                        ..Default::default()
                    },
                    spec: Some(IngressSpec {
                        ingress_class_name: Some(String::from("nginx")),
                        rules: Some(vec![IngressRule {
                            http: Some(HTTPIngressRuleValue {
                                paths: vec![HTTPIngressPath {
                                    backend: IngressBackend {
                                        service: Some(IngressServiceBackend {
                                            name: String::from("schema-registry"),
                                            port: Some(ServiceBackendPort {
                                                name: Some(String::from("http")),
                                                ..Default::default()
                                            }),
                                        }),
                                        ..Default::default()
                                    },
                                    path: Some(String::from("/schema-registry")),
                                    ..Default::default()
                                }],
                            }),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                &[V1Service {
                    metadata: ObjectMeta {
                        name: Some(String::from("schema-registry")),
                        ..Default::default()
                    },
                    spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                        ports: Some(vec![ServicePort {
                            name: Some(String::from("http")),
                            port: 8081,
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
            )
            .unwrap();

            assert_json_include!(
                actual: serde_json::to_value(route).unwrap(),
                expected: serde_json::json!({
                    "spec": {
                        "routes": [{
                            "services": [{
                                "kind": "Service",
                                "name": "schema-registry",
                                "port": 8081
                            }]
                        }]
                    }
                })
            );
        }
    }
}
