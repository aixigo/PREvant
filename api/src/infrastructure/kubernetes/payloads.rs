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
use super::super::{APP_NAME_LABEL, CONTAINER_TYPE_LABEL, SERVICE_NAME_LABEL};
use crate::models::ServiceConfig;
use base64::encode;
use chrono::Utc;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Component, PathBuf};
use std::string::ToString;

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

/// Creates a JSON payload suitable for [Kubernetes' Namespaces](https://kubernetes.io/docs/tasks/administer-cluster/namespaces/)
pub fn namespace_payload(app_name: &String) -> String {
    serde_json::json!({
      "apiVersion": "v1",
      "kind": "Namespace",
      "metadata": {
        "name": app_name
      }
    })
    .to_string()
}

/// Creates a JSON payload suitable for [Kubernetes' Deployments](https://kubernetes.io/docs/concepts/workloads/controllers/deployment/)
pub fn deployment_payload(app_name: &String, service_config: &ServiceConfig) -> String {
    let env = service_config.env().map_or(Vec::new(), |env| {
        env.iter()
            .map(|env| {
                serde_json::json!({
                  "name": env.key(),
                  "value": env.value().unsecure()
                })
            })
            .collect()
    });

    let mounts = service_config.volumes().map_or(Vec::new(), |volumes| {
        volumes
            .iter()
            .map(|(path, _)| {
                serde_json::json!({
                    "name": secret_name_from_path!(path),
                    "mountPath": path.parent().unwrap()
                })
            })
            .collect()
    });

    let volumes = service_config.volumes().map_or(Vec::new(), |volumes| {
        volumes
            .iter()
            .map(|(path, _)| {
                serde_json::json!({
                        "name": secret_name_from_path!(path),
                        "secret": {
                            "secretName": format!("{}-{}-secret", app_name, service_config.service_name()),
                            "items": [{
                                "key": secret_name_from_path!(path),
                                "path": path.file_name().map_or("", |name| name.to_str().unwrap())
                            }]
                        }
                    })
            })
            .collect()
    });

    serde_json::json!({
      "apiVersion": "apps/v1",
      "kind": "Deployment",
      "metadata": {
        "name": format!("{}-{}-deployment", app_name, service_config.service_name()),
        "namespace": app_name,
        "labels": {
          APP_NAME_LABEL: app_name,
          SERVICE_NAME_LABEL: service_config.service_name(),
          CONTAINER_TYPE_LABEL: service_config.container_type().to_string()
        }
      },
      "spec": {
        "replicas": 1,
        "selector": {
          "matchLabels": {
            APP_NAME_LABEL: app_name,
            SERVICE_NAME_LABEL: service_config.service_name(),
            CONTAINER_TYPE_LABEL: service_config.container_type().to_string()
          }
        },
        "template": {
          "metadata": {
            "labels": {
              APP_NAME_LABEL: app_name,
              SERVICE_NAME_LABEL: service_config.service_name(),
              CONTAINER_TYPE_LABEL: service_config.container_type().to_string()
            },
            "annotations": {
              "date": Utc::now()
            }
          },
          "spec": {
            "containers": [
              {
                "name": service_config.service_name(),
                "image": service_config.image().to_string(),
                "imagePullPolicy": "Always",
                "env": Value::Array(env),
                "volumeMounts": Value::Array(mounts),
                "ports": [
                  {
                    "containerPort": service_config.port()
                  }
                ]
              }
            ],
            "volumes": volumes
          }
        }
      }
    })
    .to_string()
}

/// Creates a JSON payload suitable for [Kubernetes' Secrets](https://kubernetes.io/docs/concepts/configuration/secret/)
pub fn secrets_payload(
    app_name: &String,
    service_config: &ServiceConfig,
    volumes: &BTreeMap<PathBuf, String>,
) -> String {
    let secrets = volumes
        .iter()
        .map(|(path, file_content)| {
            (
                secret_name_from_path!(path),
                Value::String(encode(file_content)),
            )
        })
        .collect::<Map<String, Value>>();

    serde_json::json!({
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
    })
    .to_string()
}

/// Creates a JSON payload suitable for [Kubernetes' Services](https://kubernetes.io/docs/concepts/services-networking/service/)
pub fn service_payload(app_name: &String, service_config: &ServiceConfig) -> String {
    serde_json::json!({
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
    })
    .to_string()
}

/// Creates a payload that ensures that Traefik find the correct route in Kubernetes
///
/// See [Traefik Routers](https://docs.traefik.io/v2.0/user-guides/crd-acme/#traefik-routers)
/// for more information.
pub fn ingress_route_payload(app_name: &String, service_config: &ServiceConfig) -> String {
    serde_json::json!({
      "apiVersion": "traefik.containo.us/v1alpha1",
      "kind": "IngressRoute",
      "metadata": {
        "name": format!("{}-{}-ingress-route", app_name, service_config.service_name()),
        "namespace": app_name,
        APP_NAME_LABEL: app_name,
        SERVICE_NAME_LABEL: service_config.service_name(),
        CONTAINER_TYPE_LABEL: service_config.container_type().to_string()
      },
      "spec": {
        "entryPoints": [
          "http"
        ],
        "routes": [
          {
            "match": service_config.traefik_rule(app_name),
            "kind": "Rule",
            "services": [
              {
                "name": service_config.service_name(),
                "port": service_config.port()
              }
            ],
            "middlewares": [
              {
                "name": format!("{}-{}-middleware", app_name, service_config.service_name())
              }
            ]
          }
        ]
      }
    })
    .to_string()
}

/// Creates a payload that ensures that Traefik strips out the path prefix.
///
/// See [Traefik Routers](https://docs.traefik.io/v2.0/user-guides/crd-acme/#traefik-routers)
/// for more information.
pub fn middleware_payload(app_name: &String, service_config: &ServiceConfig) -> String {
    serde_json::json!({
      "apiVersion": "traefik.containo.us/v1alpha1",
      "kind": "Middleware",
      "metadata": {
        "name": format!("{}-{}-middleware", app_name, service_config.service_name()),
        "namespace": app_name,
      },
       "spec": service_config.traefik_middlewares(app_name)
    })
    .to_string()
}
