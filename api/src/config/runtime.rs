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
use secstr::SecUtf8;
use serde::{de, Deserialize, Deserializer};
use std::path::PathBuf;
use url::Url;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Runtime {
    Docker,
    Kubernetes(KubernetesRuntimeConfig),
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime::Docker
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KubernetesRuntimeConfig {
    #[serde(default, deserialize_with = "parse_url")]
    endpoint: Option<Url>,
    token: Option<SecUtf8>,
    cert_auth_file_path: Option<PathBuf>,
}

impl KubernetesRuntimeConfig {
    pub fn endpoint(&self) -> &Option<Url> {
        &self.endpoint
    }

    pub fn token(&self) -> &Option<SecUtf8> {
        &self.token
    }

    pub fn cert_auth_file_path(&self) -> &Option<PathBuf> {
        &self.cert_auth_file_path
    }
}

fn parse_url<'de, D>(deserializer: D) -> Result<Option<Url>, D::Error>
where
    D: Deserializer<'de>,
{
    let url = String::deserialize(deserializer)?;
    Url::parse(&url)
        .map(|url| Some(url))
        .map_err(de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! parse_as_kubernetes_config {
        ($toml:expr) => {
            match toml::de::from_str::<Runtime>($toml).unwrap() {
                Runtime::Kubernetes(kubernetes) => kubernetes,
                _ => panic!("Should be a kubernetes config"),
            }
        };
    }

    #[test]
    fn should_parse_as_docker_runtime() {
        let runtime_toml = r#"
        type = 'Docker'
        "#;

        let runtime = toml::de::from_str::<Runtime>(runtime_toml).unwrap();

        assert_eq!(runtime, Runtime::Docker);
    }

    #[test]
    fn should_parse_as_kubernetes_runtime_without_endpoint() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        "#;

        let kubernetes = parse_as_kubernetes_config!(runtime_toml);

        assert!(kubernetes.endpoint().is_none());
    }

    #[test]
    fn should_parse_as_kubernetes_runtime_with_endpoint() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        endpoint = 'http://cluster.localhost:8080'
        "#;

        let kubernetes = parse_as_kubernetes_config!(runtime_toml);

        assert_eq!(
            kubernetes
                .endpoint()
                .as_ref()
                .map(|endpoint| endpoint.clone().to_string()),
            Some(String::from("http://cluster.localhost:8080/"))
        );
    }

    #[test]
    fn should_parse_as_kubernetes_runtime_without_token() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        "#;

        let kubernetes = parse_as_kubernetes_config!(runtime_toml);

        assert!(kubernetes.token().is_none());
    }

    #[test]
    fn should_parse_as_kubernetes_runtime_with_token() {
        let runtime_toml = r#"
        type = 'Kubernetes'
        token = 'somethingrandom'
        "#;

        let kubernetes = parse_as_kubernetes_config!(runtime_toml);

        assert_eq!(
            kubernetes
                .token()
                .as_ref()
                .map(|token| String::from(token.unsecure())),
            Some(String::from("somethingrandom"))
        );
    }
}
