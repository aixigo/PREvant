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

use regex::Regex;
use serde::ser::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::str::FromStr;
use url::Url;

#[derive(Clone)]
pub struct Service {
    app_name: String,
    service_name: String,
    container_type: ContainerType,
    container_id: String,
    base_url: Option<Url>,
}

#[derive(Clone, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    service_name: String,
    image_repository: String,
    registry: Option<String>,
    image_user: Option<String>,
    image_tag: Option<String>,
    env: Option<Vec<String>>,
    volumes: Option<BTreeMap<String, String>>,
    #[serde(skip)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(skip, default = "ContainerType::default")]
    container_type: ContainerType,
    #[serde(skip)]
    port: u16,
}

pub enum Image {
    Named {
        image_repository: String,
        registry: Option<String>,
        image_user: Option<String>,
        image_tag: Option<String>,
    },
    Digest {
        hash: String,
    },
}

impl ServiceConfig {
    pub fn new(service_name: String, image: &Image) -> ServiceConfig {
        let (image_repository, registry, image_user, image_tag) = match image {
            Image::Digest { hash } => (hash.clone(), None, None, None),
            Image::Named {
                image_repository,
                registry,
                image_user,
                image_tag,
            } => (
                image_repository.clone(),
                registry.clone(),
                image_user.clone(),
                image_tag.clone(),
            ),
        };

        ServiceConfig {
            service_name,
            image_repository,
            registry,
            image_user,
            image_tag,
            env: None,
            volumes: None,
            labels: None,
            container_type: ContainerType::Instance,
            port: 80,
        }
    }

    pub fn set_container_type(&mut self, container_type: ContainerType) {
        self.container_type = container_type;
    }

    pub fn get_container_type(&self) -> &ContainerType {
        &self.container_type
    }

    fn refers_to_image_id(&self) -> bool {
        let regex = Regex::new(r"^(sha256:)?(?P<id>[a-fA-F0-9]+)$").unwrap();
        regex
            .captures(&self.image_repository)
            .map(|_| true)
            .unwrap_or_else(|| false)
    }

    /// Returns a fully qualifying docker image
    pub fn get_image(&self) -> Image {
        if self.refers_to_image_id() {
            return Image::Digest {
                hash: self.image_repository.clone(),
            };
        }

        Image::Named {
            image_repository: self.image_repository.clone(),
            registry: self.registry.clone(),
            image_user: self.image_user.clone(),
            image_tag: self.image_tag.clone(),
        }
    }

    pub fn set_service_name(&mut self, service_name: &String) {
        self.service_name = service_name.clone()
    }

    pub fn get_service_name(&self) -> &String {
        &self.service_name
    }

    pub fn set_env(&mut self, env: Option<Vec<String>>) {
        self.env = env;
    }

    pub fn get_env<'a, 'b: 'a>(&'b self) -> Option<&'a Vec<String>> {
        match &self.env {
            None => None,
            Some(env) => Some(&env),
        }
    }

    pub fn set_labels(&mut self, labels: Option<BTreeMap<String, String>>) {
        self.labels = labels;
    }

    pub fn get_labels<'a, 'b: 'a>(&'b self) -> Option<&'a BTreeMap<String, String>> {
        match &self.labels {
            None => None,
            Some(labels) => Some(&labels),
        }
    }

    pub fn set_volumes(&mut self, volumes: Option<BTreeMap<String, String>>) {
        self.volumes = volumes;
    }

    pub fn get_volumes<'a, 'b: 'a>(&'b self) -> Option<&'a BTreeMap<String, String>> {
        match &self.volumes {
            None => None,
            Some(volumes) => Some(&volumes),
        }
    }

    pub fn set_port(&mut self, port: u16) {
        self.port = port;
    }

    pub fn get_port(&self) -> u16 {
        self.port
    }
}

impl Service {
    pub fn new(
        app_name: String,
        service_name: String,
        container_id: String,
        container_type: ContainerType,
    ) -> Service {
        Service {
            app_name,
            service_name,
            container_id,
            container_type,
            base_url: None,
        }
    }

    pub fn set_app_name(&mut self, app_name: &String) {
        self.app_name = app_name.clone();
    }

    pub fn set_base_url(&mut self, base_url: &Url) {
        self.base_url = Some(base_url.clone());
    }

    pub fn set_container_type(&mut self, container_type: ContainerType) {
        self.container_type = container_type;
    }

    fn get_base_url(&self) -> Url {
        match &self.base_url {
            None => Url::parse("http://example.org").unwrap(),
            Some(base_url) => base_url.clone(),
        }
    }

    fn get_root_url(&self) -> String {
        let mut base = self.get_base_url();
        base.set_path(&format!("/{}/{}/", &self.app_name, &self.service_name));
        base.into_string()
    }

    pub fn get_service_name(&self) -> &String {
        &self.service_name
    }

    pub fn get_container_id(&self) -> &String {
        &self.container_id
    }

    pub fn get_container_type(&self) -> &ContainerType {
        &self.container_type
    }
}

impl Serialize for Service {
    fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Service<'a> {
            vhost: &'a String,
            url: String,
            container_type: String,
        }

        let s = Service {
            vhost: &self.service_name,
            url: self.get_root_url(),
            container_type: self.container_type.to_string(),
        };

        Ok(s.serialize(serializer)?)
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize)]
pub enum ContainerType {
    #[serde(rename = "instance")]
    Instance,
    #[serde(rename = "replica")]
    Replica,
    #[serde(rename = "app-companion")]
    ApplicationCompanion,
    #[serde(rename = "service-companion")]
    ServiceCompanion,
}

impl ContainerType {
    fn default() -> ContainerType {
        ContainerType::Instance
    }
}

impl FromStr for ContainerType {
    type Err = ServiceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "replica" => Ok(ContainerType::Replica),
            "instance" => Ok(ContainerType::Instance),
            "app-companion" => Ok(ContainerType::ApplicationCompanion),
            "service-companion" => Ok(ContainerType::ServiceCompanion),
            label => Err(ServiceError::InvalidServiceType {
                label: String::from(label),
            }),
        }
    }
}

impl std::fmt::Display for ContainerType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            &ContainerType::Instance => write!(f, "instance"),
            &ContainerType::Replica => write!(f, "replica"),
            &ContainerType::ApplicationCompanion => write!(f, "app-companion"),
            &ContainerType::ServiceCompanion => write!(f, "service-companion"),
        }
    }
}

#[derive(Debug, Fail)]
pub enum ServiceError {
    #[fail(display = "Invalid service type label: {}", label)]
    InvalidServiceType { label: String },
    #[fail(
        display = "Service name {:?} does not match pattern ((.+)-.+).",
        invalid_name
    )]
    InvalidServiceName { invalid_name: String },
    #[fail(display = "Invalid image: {}", invalid_string)]
    InvalidImageString { invalid_string: String },
}

impl Image {
    pub fn get_tag(&self) -> Option<String> {
        match &self {
            Image::Digest { .. } => None,
            Image::Named {
                image_repository: _,
                registry: _,
                image_user: _,
                image_tag,
            } => match &image_tag {
                None => Some(String::from("latest")),
                Some(tag) => Some(tag.clone()),
            },
        }
    }

    pub fn get_name(&self) -> Option<String> {
        match &self {
            Image::Digest { .. } => None,
            Image::Named {
                image_repository,
                registry: _,
                image_user,
                image_tag: _,
            } => {
                let user = match &image_user {
                    None => String::from("library"),
                    Some(user) => user.clone(),
                };

                Some(format!("{}/{}", user, image_repository))
            }
        }
    }
}

/// Parse a docker image string and returns an image
impl std::str::FromStr for Image {
    type Err = ServiceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut regex = Regex::new(r"^(sha256:)?(?P<id>[a-fA-F0-9]+)$").unwrap();
        if let Some(_captures) = regex.captures(s) {
            return Ok(Image::Digest {
                hash: s.to_string(),
            });
        }

        regex = Regex::new(
            r"^(((?P<registry>.+)/)?(?P<user>[\w-]+)/)?(?P<repo>[\w-]+)(:(?P<tag>[\w\.-]+))?$",
        )
        .unwrap();
        let captures = match regex.captures(s) {
            Some(captures) => captures,
            None => {
                return Err(ServiceError::InvalidImageString {
                    invalid_string: s.to_string(),
                });
            }
        };

        let repo = captures
            .name("repo")
            .map(|m| String::from(m.as_str()))
            .unwrap();
        let registry = captures.name("registry").map(|m| String::from(m.as_str()));
        let user = captures.name("user").map(|m| String::from(m.as_str()));
        let tag = captures.name("tag").map(|m| String::from(m.as_str()));

        Ok(Image::Named {
            image_repository: repo,
            registry,
            image_user: user,
            image_tag: tag,
        })
    }
}

impl std::string::ToString for Image {
    fn to_string(&self) -> String {
        match &self {
            Image::Digest { hash } => hash.clone(),
            Image::Named {
                image_repository,
                registry,
                image_user,
                image_tag,
            } => {
                let registry = match &registry {
                    None => String::from("docker.io"),
                    Some(registry) => registry.clone(),
                };

                let user = match &image_user {
                    None => String::from("library"),
                    Some(user) => user.clone(),
                };

                let tag = match &image_tag {
                    None => "latest".to_owned(),
                    Some(tag) => tag.clone(),
                };

                format!("{}/{}/{}:{}", registry, user, image_repository, tag)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn should_parse_image_id_with_sha_prefix() {
        let image = Image::from_str(
            "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913",
        )
        .unwrap();

        assert_eq!(
            &image.to_string(),
            "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913"
        );
        assert_eq!(image.get_name(), None);
        assert_eq!(image.get_tag(), None);
    }

    #[test]
    fn should_parse_image_id() {
        let image = Image::from_str("9895c9b90b58").unwrap();

        assert_eq!(&image.to_string(), "9895c9b90b58");
        assert_eq!(image.get_name(), None);
        assert_eq!(image.get_tag(), None);
    }

    #[test]
    fn should_parse_image_with_repo_and_user() {
        let image = Image::from_str("zammad/zammad-docker-compose").unwrap();

        assert_eq!(&image.get_name().unwrap(), "zammad/zammad-docker-compose");
        assert_eq!(&image.get_tag().unwrap(), "latest");
    }

    #[test]
    fn should_parse_image_with_version() {
        let image = Image::from_str("mariadb:10.3").unwrap();

        assert_eq!(&image.get_name().unwrap(), "library/mariadb");
        assert_eq!(&image.get_tag().unwrap(), "10.3");
        assert_eq!(&image.to_string(), "docker.io/library/mariadb:10.3");
    }

    #[test]
    fn should_parse_image_with_latest_version() {
        let image = Image::from_str("nginx:latest").unwrap();

        assert_eq!(&image.get_name().unwrap(), "library/nginx");
        assert_eq!(&image.get_tag().unwrap(), "latest");
        assert_eq!(&image.to_string(), "docker.io/library/nginx:latest");
    }

    #[test]
    fn should_parse_image_with_all_information() {
        let image = Image::from_str("docker.io/library/nginx:latest").unwrap();

        assert_eq!(&image.to_string(), "docker.io/library/nginx:latest");
    }
}
