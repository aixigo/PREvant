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
use dkregistry::errors::Error as DKRegistryError;
use dkregistry::v2::manifest::Manifest;
use dkregistry::v2::Client;
use futures::future::join_all;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::convert::From;
use std::io::Error as IOError;
use std::str::FromStr;

pub struct ImagesService<'a> {
    config: &'a Config,
}

impl<'a> ImagesService<'a> {
    pub fn new<'b: 'a>(config: &'b Config) -> Self {
        Self { config }
    }

    /// Inspects all remote images through the docker registry and resolves the exposed ports of
    /// the docker images.
    pub async fn resolve_image_infos(
        &self,
        images: &HashSet<Image>,
    ) -> Result<HashMap<Image, ImageInfo>, ImagesServiceError> {
        let futures = images
            .iter()
            .filter_map(|image| match image {
                Image::Named { .. } => Some(ImagesService::resolve_image_info(self.config, image)),
                Image::Digest { .. } => None,
            })
            .collect::<Vec<_>>();
        let blobs = join_all(futures).await;

        Ok(blobs
            .into_iter()
            .filter_map(|result| match result {
                Ok((image, Some(image_info))) => Some((Image::clone(image), image_info)),
                Ok(_) => None,
                Err((image, err)) => {
                    warn!("Cannot resolve manifest of image {image}: {err}");
                    None
                }
            })
            .collect::<HashMap<Image, ImageInfo>>())
    }

    async fn resolve_image_info<'i>(
        config: &Config,
        image: &'i Image,
    ) -> Result<(&'i Image, Option<ImageInfo>), (&'i Image, ImagesServiceError)> {
        debug!("Resolve image manifest for {:?}", image);

        let client = Self::create_client(config, image)
            .await
            .map_err(|err| (image, err))?;

        let (image_name, tag) = (image.name().unwrap(), image.tag().unwrap());

        let manifest = client
            .get_manifest(&image_name, &tag)
            .await
            .map_err(|err| (image, err.into()))?;
        let blob = match manifest {
            Manifest::S2(schema) => {
                let digest = schema.manifest_spec.config().digest.to_string();
                let raw_blob = client
                    .get_blob(&image_name, &digest)
                    .await
                    .map_err(|err| (image, err.into()))?;
                match serde_json::from_str::<ImageBlob>(&String::from_utf8(raw_blob).unwrap()) {
                    Ok(blob) => Some(ImageInfo {
                        blob: Some(blob),
                        digest,
                    }),
                    Err(err) => {
                        warn!("Cannot resolve manifest for {}: {}", image, err);
                        Some(ImageInfo { blob: None, digest })
                    }
                }
            }
            _ => {
                warn!("Image of {} is not stored in Manifest::S2 format", image);
                None
            }
        };

        Ok((image, blob))
    }

    async fn create_client(config: &Config, image: &Image) -> Result<Client, ImagesServiceError> {
        let registry = image.registry().unwrap();
        let name = image.name().unwrap();
        let dk_config = dkregistry::v2::Client::configure().registry(&registry);

        match config.registry_credentials(&registry) {
            Some((username, password)) => {
                let client = dk_config
                    .username(Some(String::from(username)))
                    .password(Some(password.unsecure().to_string()))
                    .build()?;

                debug!("Login to registry: {registry}");
                let client = client
                    .authenticate(&[&format!("repository:{name}:pull")])
                    .await?;
                debug!("Logged in to registry: {registry}");

                Ok(client)
            }
            None => Ok(dk_config.build()?),
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

        ports
            .iter()
            .map(|(k, _)| k)
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
        volumes.iter().map(|(k, _v)| k).collect::<Vec<&String>>()
    }
}

#[derive(Debug, Clone, Fail)]
pub enum ImagesServiceError {
    #[fail(display = "Unexpected docker registry error: {}", internal_message)]
    UnexpectedError { internal_message: String },
    #[fail(display = "Unexpected docker image blob format: {}", internal_message)]
    InvalidImageBlob { internal_message: String },
}

impl From<IOError> for ImagesServiceError {
    fn from(e: IOError) -> Self {
        ImagesServiceError::UnexpectedError {
            internal_message: format!("{}", e),
        }
    }
}

impl From<DKRegistryError> for ImagesServiceError {
    fn from(e: DKRegistryError) -> Self {
        ImagesServiceError::UnexpectedError {
            internal_message: format!("{}", e),
        }
    }
}

impl From<serde_json::Error> for ImagesServiceError {
    fn from(e: serde_json::Error) -> Self {
        ImagesServiceError::InvalidImageBlob {
            internal_message: format!("{}", e),
        }
    }
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
