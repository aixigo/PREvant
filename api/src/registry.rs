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

use crate::config::Config;
use crate::models::Image;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use log::{debug, warn};
use oci_client::client::ClientConfig;
use oci_client::errors::OciDistributionError;
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::convert::From;
use std::str::FromStr;

pub struct Registry<'a> {
    config: &'a Config,
}

impl<'a> Registry<'a> {
    pub fn new<'b: 'a>(config: &'b Config) -> Self {
        Self { config }
    }

    /// Inspects all remote images through the docker registry and resolves the exposed ports of
    /// the docker images.
    pub async fn resolve_image_infos(
        &self,
        images: &HashSet<Image>,
    ) -> Result<HashMap<Image, ImageInfo>, RegistryError> {
        let mut resolve_image_info_futures = images
            .iter()
            .filter_map(|image| match image {
                Image::Named { .. } => Some(Registry::resolve_image_info(self.config, image)),
                Image::Digest { .. } => None,
            })
            .map(Box::pin)
            .collect::<FuturesUnordered<_>>();

        let mut image_infos = HashMap::new();
        while let Some(result) = resolve_image_info_futures.next().await {
            match result {
                Ok((image, image_info)) => {
                    image_infos.insert(image.clone(), image_info);
                }
                Err((image, err)) => {
                    return Err(match err {
                        OciDistributionError::AuthenticationFailure(err) => {
                            RegistryError::AuthenticationFailure {
                                image: image.to_string(),
                                failure: err,
                            }
                        }
                        OciDistributionError::ImageManifestNotFoundError(_) => {
                            RegistryError::ImageNotFound {
                                image: image.to_string(),
                            }
                        }
                        err => RegistryError::UnexpectedError {
                            image: image.to_string(),
                            err: anyhow::Error::new(err),
                        },
                    });
                }
            }
        }

        Ok(image_infos)
    }

    async fn resolve_image_info<'i>(
        config: &Config,
        image: &'i Image,
    ) -> Result<(&'i Image, ImageInfo), (&'i Image, OciDistributionError)> {
        debug!("Resolve image manifest for {:?}", image);

        let client = Client::new(ClientConfig {
            platform_resolver: Some(Box::new(|entries| {
                oci_client::client::current_platform_resolver(entries).or(
                    // There are cases where current_platform_resolver fails, e.g. in tests on
                    // MacOS. However it is not safe to assume the current platform that PREvant
                    // runs on it the platform the backend (Docker or Kubernetes) runs on. For
                    // example, it could be the case that clusters have multiple architectures
                    // https://carlosedp.medium.com/building-a-hybrid-x86-64-and-arm-kubernetes-cluster-e7f94ff6e51d
                    //
                    // Thus, the first entry will be used when current_platform_resolver fails and
                    // it is assumed that the information provided by the image config is equal on
                    // each platform. If a port mapping or a volume definition is different for
                    // different platforms, that would cripple into issues not just for PREvant but
                    // rather for all users that migrate to a different architecture.
                    entries.first().map(|e| e.digest.clone()),
                )
            })),
            ..Default::default()
        });

        let mut reference = Reference::from_str(&image.to_string())
            .expect("Image should be convertable if it is the Named variant");

        if let Some(mirror) = config.registry_mirror(reference.registry()) {
            reference.set_mirror_registry(mirror.to_string());
        }

        let (_manifest, digest, config) = client
            .pull_manifest_and_config(&reference, &Self::registry_auth(config, &reference))
            .await
            .map_err(|err| (image, err))?;

        let blob = match serde_json::from_str::<ImageBlob>(&config) {
            Ok(blob) => ImageInfo {
                blob: Some(blob),
                digest,
            },
            Err(err) => {
                warn!("Cannot parse manifest blob for {image}: {err}");
                ImageInfo { blob: None, digest }
            }
        };

        Ok((image, blob))
    }

    fn registry_auth(config: &Config, reference: &Reference) -> RegistryAuth {
        match config.registry_credentials(reference.registry()) {
            Some((username, password)) => {
                RegistryAuth::Basic(String::from(username), password.unsecure().to_string())
            }
            None => RegistryAuth::Anonymous,
        }
    }
}

#[derive(Debug)]
pub struct ImageInfo {
    blob: Option<ImageBlob>,
    digest: String,
}

impl ImageInfo {
    pub fn exposed_port(&self) -> Option<u16> {
        self.blob.as_ref()?.exposed_port()
    }

    pub fn digest(&self) -> &String {
        &self.digest
    }

    pub fn declared_volumes(&self) -> Vec<&String> {
        match self.blob.as_ref() {
            Some(info) => info.declared_volumes(),
            None => Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ImageBlob {
    config: ImageConfig,
}

impl ImageBlob {
    pub fn exposed_port(&self) -> Option<u16> {
        self.config.exposed_port()
    }

    pub fn declared_volumes(&self) -> Vec<&String> {
        self.config.declared_volumes()
    }
}

#[derive(Debug, Deserialize)]
struct ImageConfig {
    #[serde(rename = "ExposedPorts")]
    exposed_ports: Option<HashMap<String, serde_json::Value>>,
    #[serde(rename = "Volumes")]
    declared_volumes: Option<HashMap<String, serde_json::Value>>,
}

impl ImageConfig {
    fn exposed_port(&self) -> Option<u16> {
        let regex = Regex::new(r"^(?P<port>\d+)/(tcp|udp)$").unwrap();

        let ports = match &self.exposed_ports {
            Some(ports) => ports,
            None => return None,
        };

        ports.keys()
            .filter_map(|port| regex.captures(port))
            .filter_map(|captures| captures.name("port"))
            .filter_map(|port| u16::from_str(port.as_str()).ok())
            .min()
    }

    fn declared_volumes(&self) -> Vec<&String> {
        let volumes = match &self.declared_volumes {
            Some(volumes) => volumes,
            None => return Vec::new(),
        };
        volumes.keys().collect::<Vec<&String>>()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("Unexpected docker registry error when resolving manifest for {image}: {err}")]
    UnexpectedError { image: String, err: anyhow::Error },
    #[error("Cannot resolve image {image} due to authentication failure: {failure}")]
    AuthenticationFailure { image: String, failure: String },
    #[error("Cannot find image {image}")]
    ImageNotFound { image: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_return_exposed_port() {
        let blob = serde_json::from_str::<ImageBlob>(
            r#"{
                "config": {
                    "Hostname": "837a64dcc771",
                    "Domainname": "",
                    "User": "",
                    "AttachStdin": false,
                    "AttachStdout": false,
                    "AttachStderr": false,
                    "ExposedPorts": {
                      "8080/tcp": {},
                      "9080/udp": {}
                    }
                } }"#,
        )
        .unwrap();

        assert_eq!(blob.exposed_port(), Some(8080u16));
    }

    #[test]
    fn should_return_exposed_port_without_ports() {
        let blob = serde_json::from_str::<ImageBlob>(
            r#"{
                "config": {
                    "Hostname": "837a64dcc771",
                    "Domainname": "",
                    "User": "",
                    "AttachStdin": false,
                    "AttachStdout": false,
                    "AttachStderr": false
                } }"#,
        )
        .unwrap();

        assert_eq!(blob.exposed_port(), None);
    }

    #[test]
    fn should_return_declared_volumes() {
        let blob = serde_json::from_str::<ImageBlob>(
            r#"{
                "config": {
                    "Hostname": "837a64dcc771",
                    "Domainname": "",
                    "User": "",
                    "AttachStdin": false,
                    "AttachStdout": false,
                    "AttachStderr": false,
                    "ExposedPorts": {
                      "8080/tcp": {},
                      "9080/udp": {}
                    },
                    "Volumes": {
                       "var/lib/data" :{}
                    }
                } }"#,
        )
        .unwrap();

        assert_eq!(blob.declared_volumes(), vec!["var/lib/data"]);
    }

    #[test]
    fn should_return_none_if_no_declared_volumes() {
        let blob = serde_json::from_str::<ImageBlob>(
            r#"{
                "config": {
                    "Hostname": "837a64dcc771",
                    "Domainname": "",
                    "User": "",
                    "AttachStdin": false,
                    "AttachStdout": false,
                    "AttachStderr": false,
                    "ExposedPorts": {
                      "8080/tcp": {},
                      "9080/udp": {}
                    }
                } }"#,
        )
        .unwrap();

        assert!(blob.declared_volumes().is_empty());
    }
}
