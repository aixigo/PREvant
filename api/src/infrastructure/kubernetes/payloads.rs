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
use crate::deployment::deployment_unit::{DeployableService, DeploymentStrategy};
use crate::infrastructure::traefik::TraefikMiddleware;
use crate::infrastructure::{TraefikIngressRoute, TraefikRouterRule};
use crate::models::{AppName, ServiceConfig};
use base64::{engine::general_purpose, Engine};
use bytesize::ByteSize;
use chrono::Utc;
use k8s_openapi::api::apps::v1::DeploymentSpec;
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, KeyToPath, PersistentVolumeClaim, PersistentVolumeClaimSpec,
    PersistentVolumeClaimVolumeSource, PodSpec, PodTemplateSpec, ResourceRequirements,
    SecretVolumeSource, Volume, VolumeMount,
};
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::api::{
    apps::v1::Deployment as V1Deployment, core::v1::Namespace as V1Namespace,
    core::v1::Secret as V1Secret, core::v1::Service as V1Service,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use k8s_openapi::ByteString;
use kube::core::ObjectMeta;
use kube::CustomResource;
use multimap::MultiMap;
use schemars::JsonSchema;
use secstr::SecUtf8;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::TryFrom;
use std::hash::Hasher;
use std::iter::FromIterator;
use std::path::{Component, PathBuf};
use std::str::FromStr;
use std::string::ToString;

#[derive(CustomResource, Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "traefik.containo.us",
    version = "v1alpha1",
    kind = "IngressRoute",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct IngressRouteSpec {
    pub entry_points: Option<Vec<String>>,
    pub routes: Option<Vec<TraefikRuleSpec>>,
    pub tls: Option<TraefikTls>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TraefikRuleSpec {
    pub kind: String,
    pub r#match: String,
    pub services: Vec<TraefikRuleService>,
    pub middlewares: Option<Vec<TraefikRuleMiddleware>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TraefikRuleService {
    pub kind: Option<String>,
    pub name: String,
    pub port: Option<u16>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TraefikRuleMiddleware {
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TraefikTls {
    pub cert_resolver: Option<String>,
}

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "traefik.containo.us",
    version = "v1alpha1",
    kind = "Middleware",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct MiddlewareSpec(pub Value);

macro_rules! secret_name_from_path {
    ($path:expr) => {{
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
    ($path:expr) => {{
        $path
            .file_name()
            .map(|name| name.to_os_string().into_string().unwrap())
            .map(|name| name.replace(".", "-"))
            .unwrap_or_else(String::new)
    }};
}

impl TryFrom<IngressRoute> for TraefikIngressRoute {
    type Error = String;

    fn try_from(value: IngressRoute) -> Result<Self, Self::Error> {
        let Some(routes) = value.spec.routes else {
            return Err(String::from(
                "The ingress route does not provide any routes",
            ));
        };
        let Some(k8s_route) = routes.into_iter().next() else {
            return Err(String::from(
                "The ingress route does not provide any routes",
            ));
        };
        let rule = TraefikRouterRule::from_str(&k8s_route.r#match)?;

        Ok(TraefikIngressRoute::with_existing_routing_rules(
            value.spec.entry_points.unwrap_or_default(),
            rule,
            k8s_route
                .middlewares
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.name)
                .collect(),
            value.spec.tls.unwrap_or_default().cert_resolver,
        ))
    }
}

pub fn convert_k8s_ingress_to_traefik_ingress(
    ingress: Ingress,
    base_route: TraefikIngressRoute,
) -> Result<(IngressRoute, Option<Middleware>), &'static str> {
    let Some(spec) = ingress.spec else {
        return Err("Ingress does not provide spec");
    };
    let Some(rules) = spec.rules else {
        return Err("Ingress' spec does not provide rules");
    };

    let Some(path) = rules
        .into_iter()
        .filter_map(|rule| rule.http)
        .find_map(|http| http.paths.into_iter().next())
    else {
        return Err("Ingress' rule does not a provide http paths object");
    };

    let Some(path_value) = path.path else {
        return Err("Ingress' path does not provide a HTTP path value");
    };

    let (rule, middleware) = match &spec.ingress_class_name {
        Some(ingress_class_name) if ingress_class_name == "nginx" => {
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
                    let hir = regex_syntax::parse(&path_value).ok()?;
                    let got = regex_syntax::hir::literal::Extractor::new().extract(&hir);
                    let prefixes = got
                        .literals()?
                        .iter()
                        .map(|l| String::from_utf8_lossy(l.as_bytes()).to_string())
                        .map(serde_json::Value::from)
                        .collect::<Vec<_>>();

                    Some(Middleware {
                        metadata: kube::core::ObjectMeta {
                            name: Some(uuid::Uuid::new_v4().to_string()),
                            ..Default::default()
                        },
                        spec: MiddlewareSpec(serde_json::json!({
                            "stripPrefix": {
                                "prefixes": serde_json::Value::from(prefixes)
                            }
                        })),
                    })
                });

            (None, middleware)
        }
        _ => {
            // TODO warn that ingress class is unknown
            (
                Some(TraefikIngressRoute::with_rule(
                    TraefikRouterRule::path_prefix_rule([path_value.clone()]),
                )),
                None,
            )
        }
    };

    let mut route = base_route;
    if let Some(rule) = rule {
        route.merge_with(rule);
    }

    let mut middlewares = route
        .routes()
        .iter()
        .flat_map(|route| route.middlewares().iter())
        .filter_map(|middleware| match middleware {
            crate::infrastructure::traefik::TraefikMiddleware::Ref(name) => {
                Some(TraefikRuleMiddleware { name: name.clone() })
            }
            crate::infrastructure::traefik::TraefikMiddleware::Spec { .. } => None,
        })
        .collect::<Vec<_>>();
    middlewares.extend(middleware.as_ref().map(|m| TraefikRuleMiddleware {
        name: m.metadata.name.clone().unwrap_or_default(),
    }));

    let routes = vec![TraefikRuleSpec {
        kind: String::from("Rule"),
        r#match: route.routes()[0].rule().to_string(),
        middlewares: Some(middlewares),
        services: vec![TraefikRuleService {
            kind: Some(String::from("Service")),
            name: path.backend.service.clone().unwrap().name,
            port: Some(
                path.backend
                    .service
                    .as_ref()
                    .and_then(|service| service.port.as_ref())
                    .and_then(|port| port.number)
                    .map(|p| p as u16)
                    // TODO: how to get the if missing
                    .unwrap_or(80),
            ),
        }],
    }];

    let route = IngressRoute {
        metadata: ingress.metadata,
        spec: IngressRouteSpec {
            routes: Some(routes),
            entry_points: Some(route.entry_points().clone()),
            tls: route.tls().as_ref().map(|tls| TraefikTls {
                cert_resolver: Some(tls.cert_resolver.clone()),
            }),
        },
    };

    Ok((route, middleware))
}

/// Creates a JSON payload suitable for [Kubernetes'
/// Namespaces](https://kubernetes.io/docs/tasks/administer-cluster/namespaces/)
pub fn namespace_payload(app_name: &AppName, config: &Config) -> V1Namespace {
    let annotations = match config.runtime_config() {
        crate::config::Runtime::Docker => None,
        crate::config::Runtime::Kubernetes(runtime) => {
            let annotations = runtime.annotations().namespace();

            if annotations.is_empty() {
                None
            } else {
                Some(annotations.clone())
            }
        }
    };

    V1Namespace {
        metadata: ObjectMeta {
            name: Some(app_name.to_rfc1123_namespace_id()),
            annotations,
            labels: Some(BTreeMap::from([(
                APP_NAME_LABEL.to_string(),
                app_name.to_string(),
            )])),
            ..Default::default()
        },
        ..Default::default()
    }
}

impl AppName {
    /// See https://kubernetes.io/docs/concepts/overview/working-with-objects/names/#dns-label-names
    pub fn to_rfc1123_namespace_id(&self) -> String {
        self.to_string().to_lowercase()
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
    let env = service.env().map(|env| {
        env.iter()
            .map(|env| EnvVar {
                name: env.key().to_string(),
                value: Some(env.value().unsecure().to_string()),
                ..Default::default()
            })
            .collect()
    });

    let annotations = if let Some(replicated_env) = service
        .env()
        .and_then(super::super::replicated_environment_variable_to_json)
    {
        BTreeMap::from([
            (IMAGE_LABEL.to_string(), service.image().to_string()),
            (REPLICATED_ENV_LABEL.to_string(), replicated_env.to_string()),
        ])
    } else {
        BTreeMap::from([(IMAGE_LABEL.to_string(), service.image().to_string())])
    };

    let volume_mounts = service.files().map(|files| {
        let parent_paths = files
            .iter()
            .filter_map(|(path, _)| path.parent())
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

    let volumes = service.files().map(|files| {
        let files = files
            .iter()
            .filter_map(|(path, _)| path.parent().map(|parent| (parent, path)))
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
                            app_name,
                            service.service_name()
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

    let labels = BTreeMap::from([
        (APP_NAME_LABEL.to_string(), app_name.to_string()),
        (
            SERVICE_NAME_LABEL.to_string(),
            service.service_name().to_string(),
        ),
        (
            CONTAINER_TYPE_LABEL.to_string(),
            service.container_type().to_string(),
        ),
    ]);

    V1Deployment {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-{}-deployment",
                app_name.to_rfc1123_namespace_id(),
                service.service_name()
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
                    annotations: Some(deployment_annotations(service.strategy())),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    volumes,
                    containers: vec![Container {
                        name: service.service_name().to_string(),
                        image: Some(service.image().to_string()),
                        image_pull_policy: Some(String::from("Always")),
                        env,
                        volume_mounts,
                        ports: Some(vec![ContainerPort {
                            container_port: service.port() as i32,
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
        DeploymentStrategy::RedeployOnImageUpdate(image_id) => {
            BTreeMap::from([(String::from("imageHash"), image_id.clone())])
        }
        DeploymentStrategy::RedeployNever => BTreeMap::new(),
        DeploymentStrategy::RedeployAlways => {
            BTreeMap::from([(String::from("date"), Utc::now().to_rfc3339())])
        }
    }
}

/// Creates a JSON payload suitable for [Kubernetes' Secrets](https://kubernetes.io/docs/concepts/configuration/secret/)
pub fn secrets_payload(
    app_name: &AppName,
    service_config: &ServiceConfig,
    files: &BTreeMap<PathBuf, SecUtf8>,
) -> V1Secret {
    let secrets = files
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
        "name": format!("{}-{}-secret", app_name.to_rfc1123_namespace_id(), service_config.service_name()),
        "namespace": app_name.to_rfc1123_namespace_id(),
         APP_NAME_LABEL: app_name,
         SERVICE_NAME_LABEL: service_config.service_name(),
         CONTAINER_TYPE_LABEL: service_config.container_type().to_string()
      },
      "type": "Opaque",
      "data": secrets
    }))
    .expect("Cannot convert value to core/v1/Secret")
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
pub fn service_payload(app_name: &AppName, service_config: &ServiceConfig) -> V1Service {
    serde_json::from_value(serde_json::json!({
      "apiVersion": "v1",
      "kind": "Service",
      "namespace": app_name.to_rfc1123_namespace_id(),
      "metadata": {
        "name": service_config.service_name(),
        APP_NAME_LABEL: app_name,
        SERVICE_NAME_LABEL: service_config.service_name(),
        CONTAINER_TYPE_LABEL: service_config.container_type().to_string()
      },
      "spec": {
        "ports": [
          {
            "name": service_config.service_name(),
            "targetPort": service_config.port(),
            "port": service_config.port()
          }
        ],
        "selector": {
          APP_NAME_LABEL: app_name,
          SERVICE_NAME_LABEL: service_config.service_name(),
          CONTAINER_TYPE_LABEL: service_config.container_type().to_string()
        }
      }
    }))
    .expect("Cannot convert value to core/v1/Service")
}

/// Creates a payload that ensures that Traefik find the correct route in Kubernetes
///
/// See [Traefik Routers](https://docs.traefik.io/v2.0/user-guides/crd-acme/#traefik-routers)
/// for more information.
pub fn ingress_route_payload(app_name: &AppName, service: &DeployableService) -> IngressRoute {
    let route = service.ingress_route();

    let rules = route
        .routes()
        .iter()
        .map(|route| {
            let middlewares = route
                .middlewares()
                .iter()
                .map(|middleware| {
                    let name = match middleware {
                        crate::infrastructure::traefik::TraefikMiddleware::Ref(name) => {
                            name.clone()
                        }
                        crate::infrastructure::traefik::TraefikMiddleware::Spec {
                            name,
                            spec: _,
                        } => AppName::from_str(name)
                            .map(|app_name| app_name.to_rfc1123_namespace_id())
                            .unwrap_or_else(|_| name.clone()),
                    };
                    TraefikRuleMiddleware { name }
                })
                .collect::<Vec<_>>();

            TraefikRuleSpec {
                kind: String::from("Rule"),
                r#match: route.rule().to_string(),
                middlewares: Some(middlewares),
                services: vec![TraefikRuleService {
                    kind: Some(String::from("Service")),
                    name: service.service_name().to_string(),
                    port: Some(service.port()),
                }],
            }
        })
        .collect::<Vec<_>>();

    IngressRoute {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-{}-ingress-route",
                app_name.to_rfc1123_namespace_id(),
                service.service_name()
            )),
            namespace: Some(app_name.to_rfc1123_namespace_id()),
            annotations: Some(BTreeMap::from([
                (APP_NAME_LABEL.to_string(), app_name.to_string()),
                (
                    SERVICE_NAME_LABEL.to_string(),
                    service.service_name().to_string(),
                ),
                (
                    CONTAINER_TYPE_LABEL.to_string(),
                    service.container_type().to_string(),
                ),
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
            r.middlewares()
                .iter()
                .filter_map(|middleware| match middleware {
                    TraefikMiddleware::Ref(_) => None,
                    TraefikMiddleware::Spec { name, spec } => Some((
                        AppName::from_str(name)
                            .map(|app_name| app_name.to_rfc1123_namespace_id())
                            .unwrap_or_else(|_| name.clone()),
                        spec,
                    )),
                })
        })
        .map(|(name, spec)| Middleware {
            metadata: ObjectMeta {
                name: Some(name),
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
                service.service_name()
            )),
            labels: Some(BTreeMap::from([
                (APP_NAME_LABEL.to_owned(), app_name.to_string()),
                (
                    SERVICE_NAME_LABEL.to_owned(),
                    service.service_name().to_owned(),
                ),
                (
                    STORAGE_TYPE_LABEL.to_owned(),
                    declared_volume
                        .split('/')
                        .last()
                        .unwrap_or("default")
                        .to_owned(),
                ),
            ])),
            ..Default::default()
        },
        spec: Some(PersistentVolumeClaimSpec {
            storage_class_name: Some(storage_class.to_owned()),
            access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
            resources: Some(ResourceRequirements {
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
    use crate::infrastructure::{TraefikIngressRoute, TraefikRouterRule};
    use crate::models::{AppName, Environment, EnvironmentVariable};
    use crate::sc;
    use std::str::FromStr;

    #[test]
    fn should_create_deployment_payload() {
        let config = sc!("db", "mariadb:10.3.17");

        let payload = deployment_payload(
            &AppName::master(),
            &DeployableService::new(
                config,
                DeploymentStrategy::RedeployAlways,
                TraefikIngressRoute::with_rule(TraefikRouterRule::path_prefix_rule(&[
                    "master", "db",
                ])),
                Vec::new(),
            ),
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
        let mut config = sc!("db", "mariadb:10.3.17");
        config.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("MYSQL_ROOT_PASSWORD"),
            SecUtf8::from("example"),
        )])));

        let payload = deployment_payload(
            &AppName::master(),
            &DeployableService::new(
                config,
                DeploymentStrategy::RedeployAlways,
                TraefikIngressRoute::with_rule(TraefikRouterRule::path_prefix_rule(&[
                    "master", "db",
                ])),
                Vec::new(),
            ),
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
        let mut config = sc!("db", "mariadb:10.3.17");
        config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_replicated(
                String::from("MYSQL_ROOT_PASSWORD"),
                SecUtf8::from("example"),
            ),
        ])));

        let payload = deployment_payload(
            &AppName::master(),
            &DeployableService::new(
                config,
                DeploymentStrategy::RedeployAlways,
                TraefikIngressRoute::with_rule(TraefikRouterRule::path_prefix_rule(&[
                    "master", "db",
                ])),
                Vec::new(),
            ),
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
        let config = sc!("db", "mariadb:10.3.17");

        let payload = deployment_payload(
            &AppName::from_str("MY-APP").unwrap(),
            &DeployableService::new(
                config,
                DeploymentStrategy::RedeployAlways,
                TraefikIngressRoute::with_rule(TraefikRouterRule::path_prefix_rule(&[
                    "master", "db",
                ])),
                Vec::new(),
            ),
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
    fn should_create_ingress_route() {
        let app_name = AppName::master();
        let mut config = sc!("db", "mariadb:10.3.17");
        let port = 1234;
        config.set_port(port);
        let config = DeployableService::new(
            config,
            DeploymentStrategy::RedeployAlways,
            TraefikIngressRoute::with_defaults(&app_name, "db"),
            Vec::new(),
        );
        let payload = ingress_route_payload(&app_name, &config);

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
                        "port": port,
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
        let app_name = AppName::from_str("MY-APP").unwrap();
        let mut config = sc!("db", "mariadb:10.3.17");
        let port = 1234;
        config.set_port(port);
        let config = DeployableService::new(
            config,
            DeploymentStrategy::RedeployAlways,
            TraefikIngressRoute::with_defaults(&app_name, "db"),
            Vec::new(),
        );
        let payload = ingress_route_payload(&app_name, &config);

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
                        "port": port,
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
        let config = sc!("db", "mariadb:10.3.17");

        let persistent_volume_claim = PersistentVolumeClaim {
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
                resources: Some(ResourceRequirements {
                    requests: Some(BTreeMap::from_iter(vec![(
                        "storage".to_owned(),
                        Quantity("2Gi".to_owned()),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let payload = deployment_payload(
            &AppName::master(),
            &DeployableService::new(
                config,
                DeploymentStrategy::RedeployAlways,
                TraefikIngressRoute::with_rule(TraefikRouterRule::path_prefix_rule(&[
                    "master", "db",
                ])),
                vec![String::from("/var/lib/data")],
            ),
            &ContainerConfig::default(),
            &Some(HashMap::from([(
                &String::from("/var/lib/data"),
                persistent_volume_claim,
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
        let mut config = sc!("db", "mariadb:10.3.17");
        config.set_files(Some(BTreeMap::from([(
            PathBuf::from("/etc/mysql/my.cnf"),
            SecUtf8::from_str(
                r"[client-server]
                  socket=/tmp/mysql.sock
                  port=3306",
            )
            .unwrap(),
        )])));

        let payload = deployment_payload(
            &AppName::master(),
            &DeployableService::new(
                config,
                DeploymentStrategy::RedeployAlways,
                TraefikIngressRoute::with_rule(TraefikRouterRule::path_prefix_rule(&[
                    "master", "db",
                ])),
                Vec::new(),
            ),
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
        let namespace =
            namespace_payload(&AppName::from_str("MY-APP").unwrap(), &Default::default());

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

        let namespace = namespace_payload(&AppName::from_str("myapp").unwrap(), &config);

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
}
