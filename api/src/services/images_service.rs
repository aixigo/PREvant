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

use crate::models::Image;
use crate::models::ServiceConfig;
use dkregistry::errors::Error as DKRegistryError;
use dkregistry::v2::manifest::Manifest;
use futures::Future;
use regex::Regex;
use std::collections::HashMap;
use std::convert::From;
use std::io::Error as IOError;
use std::str::FromStr;
use tokio_core::reactor::Core;

pub struct ImagesService {}

impl ImagesService {
    pub fn new() -> ImagesService {
        ImagesService {}
    }

    /// Inspects all remote images through the docker registry and resolves the exposed ports of
    /// the docker images.
    pub fn resolve_image_ports(
        &self,
        configs: &Vec<ServiceConfig>,
    ) -> Result<HashMap<ServiceConfig, u16>, ImagesServiceError> {
        let mut core = Core::new()?;
        let mut port_mappings = HashMap::new();

        for config in configs.iter() {
            let image = config.image();

            if let Image::Digest { hash: _ } = image {
                break;
            }

            debug!("Resolve image manifest for {}", config.image().to_string());

            let client = dkregistry::v2::Client::configure()
                .registry(&image.registry().unwrap())
                .build()?;

            let image = config.image().name().unwrap();
            let tag = config.image().tag().unwrap();

            let future = futures::future::ok::<_, DKRegistryError>(client)
                .and_then(|dclient| Ok(dclient))
                .and_then(|dclient| {
                    let img = image.clone();
                    dclient
                        .get_manifest(&img, &tag)
                        .and_then(move |manifest| match manifest {
                            Manifest::S2(schema) => {
                                Ok((dclient, schema.manifest_spec.config().digest.clone()))
                            }
                            _ => Err("unknown format".into()),
                        })
                })
                .and_then(|(dclient, digest)| {
                    let img = image.clone();

                    let get_blob_future = dclient.get_blob(&img, &digest);
                    get_blob_future.map(move |blob| {
                        serde_json::from_str::<ImageBlob>(&String::from_utf8(blob).unwrap())
                    })
                });

            match core.run(future) {
                Ok(blob) => match blob {
                    Ok(blob) => {
                        if let Some(port) = blob.get_exposed_port() {
                            port_mappings.insert(config.clone(), port);
                        }
                    }
                    Err(err) => {
                        warn!("Cannot resolve manifest for {}: {}", image, err);
                    }
                },
                Err(err) => {
                    warn!("Cannot resolve manifest for {}: {}", image, err);
                }
            }
        }

        Ok(port_mappings)
    }
}

#[derive(Deserialize)]
struct ImageBlob {
    config: ImageConfig,
}

impl ImageBlob {
    pub fn get_exposed_port(&self) -> Option<u16> {
        self.config.get_exposed_port()
    }
}

#[derive(Deserialize)]
struct ImageConfig {
    #[serde(rename = "ExposedPorts")]
    exposed_ports: Option<HashMap<String, serde_json::Value>>,
}

impl ImageConfig {
    fn get_exposed_port(&self) -> Option<u16> {
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
}

#[derive(Debug, Fail)]
pub enum ImagesServiceError {
    #[fail(display = "Could not find image: {}", internal_message)]
    ImageNotFound { internal_message: String },
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

        assert_eq!(blob.get_exposed_port(), Some(8080u16));
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

        assert_eq!(blob.get_exposed_port(), None);
    }
}
