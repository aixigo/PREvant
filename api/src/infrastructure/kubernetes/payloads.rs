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
};
use crate::config::ContainerConfig;
use crate::infrastructure::DeploymentStrategy;
use crate::models::service::Service;
use crate::models::ServiceConfig;
use base64::encode;
use chrono::Utc;
use k8s_openapi::api::apps::v1::DeploymentSpec;
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, KeyToPath, LocalObjectReference, PodSpec, PodTemplateSpec,
    ResourceRequirements, SecretVolumeSource, Volume, VolumeMount,
};
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
use std::collections::{BTreeMap, HashSet};
use std::path::{Component, PathBuf};
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
    routes: Option<Vec<TraefikRuleSpec>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TraefikRuleSpec {
    kind: String,
    r#match: String,
    services: Vec<TraefikRuleService>,
    middlewares: Vec<TraefikRuleMiddleware>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TraefikRuleService {
    kind: String,
    name: String,
    port: u16,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TraefikRuleMiddleware {
    name: String,
}

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "traefik.containo.us",
    version = "v1alpha1",
    kind = "Middleware",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct MiddlewareSpec(BTreeMap<String, Value>);

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

/// Creates a JSON payload suitable for [Kubernetes' Namespaces](https://kubernetes.io/docs/tasks/administer-cluster/namespaces/)
pub fn namespace_payload(app_name: &String) -> V1Namespace {
    serde_json::from_value(serde_json::json!({
      "apiVersion": "v1",
      "kind": "Namespace",
      "metadata": {
        "name": app_name
      }
    }))
    .expect("Cannot convert value to core/v1/Namespace")
}

/// Creates a JSON payload suitable for [Kubernetes' Deployments](https://kubernetes.io/docs/concepts/workloads/controllers/deployment/)
pub fn deployment_payload(
    app_name: &str,
    strategy: &DeploymentStrategy,
    container_config: &ContainerConfig,
    use_image_pull_secret: bool,
) -> V1Deployment {
    let env = strategy.env().map(|env| {
        env.iter()
            .map(|env| EnvVar {
                name: env.key().to_string(),
                value: Some(env.value().unsecure().to_string()),
                ..Default::default()
            })
            .collect()
    });

    let annotations = if let Some(replicated_env) = strategy
        .env()
        .and_then(super::super::replicated_environment_variable_to_json)
    {
        BTreeMap::from([
            (IMAGE_LABEL.to_string(), strategy.image().to_string()),
            (REPLICATED_ENV_LABEL.to_string(), replicated_env.to_string()),
        ])
    } else {
        BTreeMap::from([(IMAGE_LABEL.to_string(), strategy.image().to_string())])
    };

    let volume_mounts = strategy.files().map(|files| {
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

    let volumes = strategy.files().map(|files| {
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
                            strategy.service_name()
                        )),
                        items: Some(items),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            })
            .collect()
    });

    let resources = container_config
        .memory_limit()
        .map(|mem_limit| ResourceRequirements {
            limits: Some(BTreeMap::from([(
                String::from("memory"),
                Quantity(format!("{mem_limit}")),
            )])),
            ..Default::default()
        });

    let labels = BTreeMap::from([
        (APP_NAME_LABEL.to_string(), app_name.to_string()),
        (
            SERVICE_NAME_LABEL.to_string(),
            strategy.service_name().to_string(),
        ),
        (
            CONTAINER_TYPE_LABEL.to_string(),
            strategy.container_type().to_string(),
        ),
    ]);

    V1Deployment {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-{}-deployment",
                app_name,
                strategy.service_name()
            )),
            namespace: Some(app_name.to_string()),
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
                    annotations: Some(deployment_annotations(strategy)),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![Container {
                        name: strategy.service_name().to_string(),
                        image: Some(strategy.image().to_string()),
                        image_pull_policy: Some(String::from("Always")),
                        env,
                        volume_mounts,
                        ports: Some(vec![ContainerPort {
                            container_port: strategy.port() as i32,
                            ..Default::default()
                        }]),
                        resources,
                        ..Default::default()
                    }],
                    volumes,
                    image_pull_secrets: if use_image_pull_secret {
                        Some(vec![LocalObjectReference {
                            name: Some(format!("{app_name}-image-pull-secret")),
                        }])
                    } else {
                        None
                    },
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
        DeploymentStrategy::RedeployOnImageUpdate(_, image_id) => {
            BTreeMap::from([(String::from("imageHash"), image_id.clone())])
        }
        DeploymentStrategy::RedeployNever(_) => BTreeMap::new(),
        DeploymentStrategy::RedeployAlways(_) => {
            BTreeMap::from([(String::from("date"), Utc::now().to_rfc3339())])
        }
    }
}

pub fn deployment_replicas_payload(
    app_name: &String,
    service: &Service,
    replicas: u32,
) -> V1Deployment {
    serde_json::from_value(serde_json::json!({
      "apiVersion": "apps/v1",
      "kind": "Deployment",
      "metadata": {
        "name": format!("{}-{}-deployment", app_name, service.service_name()),
        "namespace": app_name,
        "labels": {
          APP_NAME_LABEL: app_name,
          SERVICE_NAME_LABEL: service.service_name(),
          CONTAINER_TYPE_LABEL: service.container_type().to_string()
        }
      },
      "spec": {
        "replicas": replicas,
        "selector": {
          "matchLabels": {
            APP_NAME_LABEL: app_name,
            SERVICE_NAME_LABEL: service.service_name(),
            CONTAINER_TYPE_LABEL: service.container_type().to_string()
          }
        }
      }
    }))
    .expect("Cannot convert value to apps/v1/Deployment")
}

/// Creates a JSON payload suitable for [Kubernetes' Secrets](https://kubernetes.io/docs/concepts/configuration/secret/)
pub fn secrets_payload(
    app_name: &String,
    service_config: &ServiceConfig,
    files: &BTreeMap<PathBuf, SecUtf8>,
) -> V1Secret {
    let secrets = files
        .iter()
        .map(|(path, file_content)| {
            (
                secret_name_from_name!(path),
                Value::String(encode(file_content.unsecure())),
            )
        })
        .collect::<Map<String, Value>>();

    serde_json::from_value(serde_json::json!({
      "apiVersion": "v1",
      "kind": "Secret",
      "metadata": {
        "name": format!("{}-{}-secret", app_name, service_config.service_name()),
        "namespace": app_name,
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
    app_name: &str,
    registries_and_credentials: BTreeMap<String, (&str, &SecUtf8)>,
) -> V1Secret {
    use core::iter::FromIterator;
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
            name: Some(format!("{app_name}-image-pull-secret")),
            namespace: Some(app_name.to_string()),
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
pub fn service_payload(app_name: &String, service_config: &ServiceConfig) -> V1Service {
    serde_json::from_value(serde_json::json!({
      "apiVersion": "v1",
      "kind": "Service",
      "namespace": app_name,
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
pub fn ingress_route_payload(app_name: &String, service_config: &ServiceConfig) -> IngressRoute {
    IngressRoute {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-{}-ingress-route",
                app_name,
                service_config.service_name()
            )),
            namespace: Some(app_name.to_string()),
            annotations: Some(BTreeMap::from([
                (APP_NAME_LABEL.to_string(), app_name.to_string()),
                (
                    SERVICE_NAME_LABEL.to_string(),
                    service_config.service_name().to_string(),
                ),
                (
                    CONTAINER_TYPE_LABEL.to_string(),
                    service_config.container_type().to_string(),
                ),
                (
                    String::from("traefik.ingress.kubernetes.io/router.entrypoints"),
                    String::from("web"),
                ),
            ])),
            ..Default::default()
        },
        spec: IngressRouteSpec {
            routes: Some(vec![TraefikRuleSpec {
                kind: String::from("Rule"),
                r#match: service_config.traefik_rule(app_name),
                services: vec![TraefikRuleService {
                    kind: String::from("Service"),
                    name: service_config.service_name().to_string(),
                    port: service_config.port(),
                }],
                middlewares: vec![TraefikRuleMiddleware {
                    name: format!("{}-{}-middleware", app_name, service_config.service_name()),
                }],
            }]),
        },
    }
}

/// Creates a payload that ensures that Traefik strips out the path prefix.
///
/// See [Traefik Routers](https://docs.traefik.io/v2.0/user-guides/crd-acme/#traefik-routers)
/// for more information.
pub fn middleware_payload(app_name: &String, service_config: &ServiceConfig) -> Middleware {
    serde_json::from_value(serde_json::json!({
      "apiVersion": "traefik.containo.us/v1alpha1",
      "kind": "Middleware",
      "metadata": {
        "name": format!("{}-{}-middleware", app_name, service_config.service_name()),
        "namespace": app_name,
      },
       "spec": service_config.traefik_middlewares(app_name)
    }))
    .expect("Cannot convert value to traefik.containo.us/v1alpha1/MiddleWare")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Environment, EnvironmentVariable};
    use crate::sc;

    #[test]
    fn should_create_deployment_payload() {
        let config = sc!("db", "mariadb:10.3.17");

        let payload =
            deployment_payload("master", &config.into(), &ContainerConfig::default(), false);

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

        let payload =
            deployment_payload("master", &config.into(), &ContainerConfig::default(), false);

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

        let payload =
            deployment_payload("master", &config.into(), &ContainerConfig::default(), false);

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
    fn should_create_ingress_route() {
        let mut config = sc!("db", "mariadb:10.3.17");
        let port = 1234;
        config.set_port(port);

        let payload = ingress_route_payload(&String::from("master"), &config);

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
    fn should_create_middleware_with_default_prefix() {
        let config = sc!("db", "mariadb:10.3.17");

        let payload = middleware_payload(&String::from("master"), &config);

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
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
            }),
        );
    }

    #[test]
    fn should_create_middleware_with_extra_config() {
        let mut middlewares: BTreeMap<String, serde_value::Value> = BTreeMap::new();
        middlewares.insert(
            String::from("compress"),
            serde_value::to_value(serde_json::json!({})).expect("Should create value"),
        );
        middlewares.insert(
            String::from("rateLimit"),
            serde_value::to_value(serde_json::json!({"average": 100}))
                .expect("Should create value"),
        );
        let mut config = sc!("db", "mariadb:10.3.17");
        config.set_middlewares(middlewares);

        let payload = middleware_payload(&String::from("master"), &config);

        assert_json_diff::assert_json_include!(
            actual: payload,
            expected: serde_json::json!({
              "apiVersion": "traefik.containo.us/v1alpha1",
              "kind": "Middleware",
              "metadata": {
                "name": "master-db-middleware",
                "namespace": "master",
              },
              "spec": {
                "compress": {},
                "rateLimit": {
                  "average": 100
                },
              },
            }),
        );
    }
}
