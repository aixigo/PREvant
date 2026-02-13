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
use bytesize::ByteSize;
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[derive(Default)]
pub enum Runtime {
    #[default]
    Docker,
    Kubernetes(KubernetesRuntimeConfig),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KubernetesRuntimeConfig {
    #[serde(default)]
    pub annotations: KubernetesAnnotationsConfig,
    #[serde(default)]
    pub downward_api: KubernetesDownwardApiConfig,
    #[serde(default)]
    pub storage_config: KubernetesStorageConfig,
    #[serde(default)]
    pub kube_config: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct KubernetesAnnotationsConfig {
    #[serde(default)]
    pub namespace: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KubernetesDownwardApiConfig {
    pub labels_path: PathBuf,
}

impl Default for KubernetesDownwardApiConfig {
    fn default() -> Self {
        Self {
            labels_path: PathBuf::from("/run/podinfo/labels"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KubernetesStorageConfig {
    #[serde(default = "KubernetesStorageConfig::default_storage_size")]
    pub storage_size: ByteSize,
    pub storage_class: Option<String>,
}

impl KubernetesStorageConfig {
    fn default_storage_size() -> ByteSize {
        ByteSize::gb(2)
    }
}

impl Default for KubernetesStorageConfig {
    fn default() -> Self {
        Self {
            storage_size: Self::default_storage_size(),
            storage_class: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_from_minimal_config_as_docker_runtime() {
        let runtime_toml = r#"
        type = 'Docker'
        "#;

        let runtime = toml::de::from_str::<Runtime>(runtime_toml).unwrap();

        assert_eq!(runtime, Runtime::Docker);
    }

    #[test]
    fn parse_form_minimal_config_as_kubernetes_runtime() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        "#;

        let runtime = toml::de::from_str::<Runtime>(runtime_toml).unwrap();

        assert_eq!(runtime, Runtime::Kubernetes(Default::default()));
    }

    #[test]
    fn parse_as_kubernetes_runtime_with_label_downward_path() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        [downwardApi]
        labelsPath = '/some/path'
        "#;

        let runtime = toml::de::from_str::<Runtime>(runtime_toml).unwrap();

        assert_eq!(
            runtime,
            Runtime::Kubernetes(KubernetesRuntimeConfig {
                downward_api: KubernetesDownwardApiConfig {
                    labels_path: PathBuf::from("/some/path")
                },
                ..Default::default()
            })
        );
    }

    #[test]
    fn provide_default_labels_path() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        "#;

        let Runtime::Kubernetes(config) = toml::de::from_str::<Runtime>(runtime_toml).unwrap()
        else {
            panic!("Need a K8s config")
        };

        assert_eq!(
            config.downward_api.labels_path,
            PathBuf::from("/run/podinfo/labels")
        )
    }

    #[test]
    fn parse_as_kubernetes_storage_config() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        [storageConfig]
        storageSize = '10g'
        storageClass = 'local-path'
        "#;

        let runtime = toml::de::from_str::<Runtime>(runtime_toml).unwrap();

        assert_eq!(
            runtime,
            Runtime::Kubernetes(KubernetesRuntimeConfig {
                storage_config: KubernetesStorageConfig {
                    storage_size: ByteSize::gb(10),
                    storage_class: Some(String::from("local-path"))
                },
                ..Default::default()
            })
        );
    }

    #[test]
    fn parse_without_namespace_annotations() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        "#;

        let Runtime::Kubernetes(config) = toml::de::from_str::<Runtime>(runtime_toml).unwrap()
        else {
            panic!("Need a K8s config")
        };

        assert!(config.annotations.namespace.is_empty());
    }

    #[test]
    fn parse_with_namespace_annotations() {
        let runtime_toml = r#"
        type = 'Kubernetes'

        [annotations.namespace]
        'field.cattle.io/containerDefaultResourceLimit' = '{}'
        'field.cattle.io/projectId' = "rancher-project-id"
        'field.cattle.io/resourceQuota' = '{"limit":{"limitsMemory":"30000Mi"}}'
        "#;

        let Runtime::Kubernetes(config) = toml::de::from_str::<Runtime>(runtime_toml).unwrap()
        else {
            panic!("Need a K8s config")
        };

        assert_eq!(
            config
                .annotations
                .namespace
                .get("field.cattle.io/projectId"),
            Some(&String::from("rancher-project-id"))
        );
    }
}
