/*-
 * ========================LICENSE_START=================================
 * PREvant
 * %%
 * Copyright (C) 2018 aixigo AG
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

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    service_name: String,
    image_repository: String,
    registry: Option<String>,
    image_user: Option<String>,
    image_tag: Option<String>,
    env: Option<Vec<String>>,
    volumes: BTreeMap<String, String>,
    #[serde(skip, default = "ContainerType::default")]
    container_type: ContainerType,
}

impl ServiceConfig {
    pub fn new(
        service_name: &String,
        image_repository: &String,
        env: Option<Vec<String>>,
    ) -> ServiceConfig {
        ServiceConfig {
            service_name: service_name.clone(),
            image_repository: image_repository.clone(),
            registry: None,
            image_user: None,
            image_tag: None,
            env,
            volumes: BTreeMap::new(),
            container_type: ContainerType::Instance,
        }
    }

    pub fn set_registry(&mut self, registry: &Option<String>) {
        self.registry = registry.clone()
    }

    pub fn set_image_user(&mut self, image_user: &Option<String>) {
        self.image_user = image_user.clone()
    }

    pub fn set_image_tag(&mut self, image_tag: &Option<String>) {
        self.image_tag = image_tag.clone()
    }

    fn get_docker_image_base(&self) -> String {
        let image_user = match &self.image_user {
            Some(user) => user.clone(),
            None => String::from("library"),
        };

        format!("{}/{}", image_user, self.image_repository)
    }

    pub fn set_container_type(&mut self, container_type: ContainerType) {
        self.container_type = container_type;
    }

    pub fn get_container_type(&self) -> &ContainerType {
        &self.container_type
    }

    pub fn refers_to_image_id(&self) -> bool {
        let regex = Regex::new(r"^(sha256:)?(?P<id>[a-fA-F0-9]+)$").unwrap();
        regex
            .captures(&self.image_repository)
            .map(|_| true)
            .unwrap_or_else(|| false)
    }

    /// Returns a fully qualifying docker image string
    pub fn get_docker_image(&self) -> String {
        if self.refers_to_image_id() {
            return self.image_repository.clone();
        }

        let registry = match &self.registry {
            None => String::from("docker.io"),
            Some(registry) => registry.clone(),
        };

        format!(
            "{}/{}:{}",
            registry,
            self.get_docker_image_base(),
            self.get_image_tag()
        )
    }

    pub fn set_service_name(&mut self, service_name: &String) {
        self.service_name = service_name.clone()
    }

    pub fn get_service_name(&self) -> &String {
        &self.service_name
    }

    pub fn get_image_tag(&self) -> String {
        match &self.image_tag {
            None => "latest".to_owned(),
            Some(tag) => tag.clone(),
        }
    }

    pub fn set_env(&mut self, env: &Option<Vec<String>>) {
        self.env = env.clone();
    }

    pub fn get_env(&self) -> Option<Vec<String>> {
        match &self.env {
            None => None,
            Some(env) => Some(env.clone()),
        }
    }

    pub fn set_volumes(&mut self, volumes: &BTreeMap<String, String>) {
        self.volumes = volumes.clone();
    }

    pub fn get_volumes(&self) -> &BTreeMap<String, String> {
        &self.volumes
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ContainerType {
    Instance,
    Replica,
    ApplicationCompanion,
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

/// Parse a docker image string and returns either `imageRepository` with `imageUser`, `registry`,
/// and `imageTag` or the image id.
pub fn parse_image_string(
    image: &str,
) -> Result<(String, Option<String>, Option<String>, Option<String>), ServiceError> {
    let mut regex = Regex::new(r"^(sha256:)?(?P<id>[a-fA-F0-9]+)$").unwrap();
    if let Some(captures) = regex.captures(image) {
        return Ok((
            captures
                .name("id")
                .map(|m| String::from(m.as_str()))
                .unwrap(),
            None,
            None,
            None,
        ));
    }

    regex = Regex::new(
        r"^(((?P<registry>.+)/)?(?P<user>[\w-]+)/)?(?P<repo>[\w-]+)(:(?P<tag>[\w-]+))?$",
    )
    .unwrap();
    let captures = match regex.captures(image) {
        Some(captures) => captures,
        None => {
            return Err(ServiceError::InvalidImageString {
                invalid_string: String::from(image),
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

    Ok((repo, user, registry, tag))
}

#[cfg(test)]
mod tests {

    #[test]
    fn should_parse_image_id_with_sha_prefix() {
        let (repo, user, registry, tag) = super::parse_image_string(
            "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913",
        )
        .unwrap();

        assert_eq!(
            &repo,
            "9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913"
        );
        assert_eq!(user, None);
        assert_eq!(registry, None);
        assert_eq!(tag, None);
    }

    #[test]
    fn should_parse_image_id() {
        let (repo, user, registry, tag) = super::parse_image_string("9895c9b90b58").unwrap();

        assert_eq!(&repo, "9895c9b90b58");
        assert_eq!(user, None);
        assert_eq!(registry, None);
        assert_eq!(tag, None);
    }

    #[test]
    fn should_parse_image_with_repo_and_user() {
        let (repo, user, registry, tag) =
            super::parse_image_string("zammad/zammad-docker-compose").unwrap();

        assert_eq!(&repo, "zammad-docker-compose");
        assert_eq!(&user.unwrap(), "zammad");
        assert_eq!(registry, None);
        assert_eq!(tag, None);
    }

    #[test]
    fn should_parse_image_with_version() {
        let (repo, user, registry, tag) = super::parse_image_string("nginx:latest").unwrap();

        assert_eq!(&repo, "nginx");
        assert_eq!(user, None);
        assert_eq!(registry, None);
        assert_eq!(&tag.unwrap(), "latest");
    }

    #[test]
    fn should_parse_image_with_all_information() {
        let (repo, user, registry, tag) =
            super::parse_image_string("docker.io/library/nginx:latest").unwrap();

        assert_eq!(&repo, "nginx");
        assert_eq!(&user.unwrap(), "library");
        assert_eq!(&registry.unwrap(), "docker.io");
        assert_eq!(&tag.unwrap(), "latest");
    }

}
