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

use crate::models::web_host_meta::WebHostMeta;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::ser::{Serialize, Serializer};
use serde::{de, Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::str::FromStr;
use url::Url;

#[derive(Clone, Debug)]
pub struct Service {
    /// An unique identifier of the service, e.g. the container id
    id: String,
    app_name: String,
    service_name: String,
    container_type: ContainerType,
    base_url: Option<Url>,
    endpoint: Option<ServiceEndpoint>,
    web_host_meta: Option<WebHostMeta>,
    state: State,
}

#[derive(Clone, Debug)]
struct ServiceEndpoint {
    internal_addr: IpAddr,
    exposed_port: u16,
}

impl ServiceEndpoint {
    fn to_url(&self) -> Url {
        Url::parse(&format!(
            "http://{}:{}/",
            self.internal_addr, self.exposed_port
        ))
        .unwrap()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    service_name: String,
    #[serde(deserialize_with = "Image::parse_from_string")]
    image: Image,
    env: Option<Vec<String>>,
    // TODO: rename this field because it does not match to volumes any more (it is file content, cf. issue #8)
    volumes: Option<BTreeMap<String, String>>,
    #[serde(skip)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(skip, default = "ContainerType::default")]
    container_type: ContainerType,
    #[serde(skip)]
    port: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq)]
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    status: ServiceStatus,
    #[serde(skip)]
    started_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceStatus {
    Running,
    Paused,
}

impl ServiceConfig {
    pub fn new(service_name: String, image: Image) -> ServiceConfig {
        ServiceConfig {
            service_name,
            image,
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

    pub fn container_type(&self) -> &ContainerType {
        &self.container_type
    }

    /// Returns a fully qualifying docker image
    pub fn image(&self) -> &Image {
        &self.image
    }

    pub fn set_service_name(&mut self, service_name: &String) {
        self.service_name = service_name.clone()
    }

    pub fn service_name(&self) -> &String {
        &self.service_name
    }

    pub fn set_env(&mut self, env: Option<Vec<String>>) {
        self.env = env;
    }

    pub fn env<'a, 'b: 'a>(&'b self) -> Option<&'a Vec<String>> {
        match &self.env {
            None => None,
            Some(env) => Some(&env),
        }
    }

    pub fn set_labels(&mut self, labels: Option<BTreeMap<String, String>>) {
        self.labels = labels;
    }

    pub fn labels<'a, 'b: 'a>(&'b self) -> Option<&'a BTreeMap<String, String>> {
        match &self.labels {
            None => None,
            Some(labels) => Some(&labels),
        }
    }

    pub fn add_volume(&mut self, path: String, data: String) {
        if let Some(ref mut volumes) = self.volumes {
            volumes.insert(path, data);
        } else {
            let mut volumes = BTreeMap::new();
            volumes.insert(path, data);
            self.volumes = Some(volumes);
        }
    }

    pub fn set_volumes(&mut self, volumes: Option<BTreeMap<String, String>>) {
        self.volumes = volumes;
    }

    pub fn volumes<'a, 'b: 'a>(&'b self) -> Option<&'a BTreeMap<String, String>> {
        match &self.volumes {
            None => None,
            Some(volumes) => Some(&volumes),
        }
    }

    pub fn set_port(&mut self, port: u16) {
        self.port = port;
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Copy labels, envs and volumes from other into self.
    /// If something is defined in self and other, self has precedence.
    pub fn merge_with(&mut self, other: &Self) {
        // We parse env-strings of the form A=B. If a string is unparseable,
        // it is kept as is.
        let mut new_envs = vec![];
        let mut own_envs_map = BTreeMap::new();
        for env in self.env().into_iter().flatten() {
            let parts = env.splitn(2, "=").collect::<Vec<&str>>();
            if parts.len() == 2 {
                own_envs_map.insert(parts[0], parts[1]);
            } else {
                new_envs.push(env.to_owned());
            }
        }
        for env in other.env().into_iter().flatten() {
            let parts = env.splitn(2, "=").collect::<Vec<&str>>();
            if parts.len() == 2 {
                let own_value = own_envs_map.remove(parts[0]);
                new_envs.push(format!(
                    "{}={}",
                    parts[0],
                    String::from(own_value.unwrap_or(parts[1]))
                ));
            } else {
                new_envs.push(env.to_owned());
            }
        }
        for (key, value) in own_envs_map.iter() {
            new_envs.push(format!("{}={}", key, value));
        }
        // Reverse the envs-array to ensure that unparseable strings from self
        // are added after unparseable strings from other
        new_envs.reverse();
        self.set_env(Some(new_envs));

        let mut volumes = other.volumes().unwrap_or(&BTreeMap::new()).clone();
        volumes.extend(self.volumes().unwrap_or(&BTreeMap::new()).clone());
        self.set_volumes(Some(volumes));

        let mut labels = other.labels().unwrap_or(&BTreeMap::new()).clone();
        labels.extend(self.labels().unwrap_or(&BTreeMap::new()).clone());
        self.set_labels(Some(labels));
    }
}

impl Service {
    pub fn new(
        id: String,
        app_name: String,
        service_name: String,
        container_type: ContainerType,
        status: ServiceStatus,
        started_at: DateTime<Utc>,
    ) -> Service {
        Service {
            id,
            app_name,
            service_name,
            container_type,
            base_url: None,
            endpoint: None,
            web_host_meta: None,
            state: State { status, started_at },
        }
    }

    pub fn app_name(&self) -> &String {
        &self.app_name
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

    fn service_url(&self) -> Option<Url> {
        self.base_url.clone().map(|url| {
            url.join(&format!("/{}/{}/", &self.app_name, &self.service_name))
                .unwrap()
        })
    }

    pub fn id(&self) -> &String {
        &self.id
    }

    pub fn service_name(&self) -> &String {
        &self.service_name
    }

    pub fn container_type(&self) -> &ContainerType {
        &self.container_type
    }

    pub fn port(&self) -> Option<u16> {
        match &self.endpoint {
            None => None,
            Some(endpoint) => Some(endpoint.exposed_port),
        }
    }

    pub fn set_endpoint(&mut self, addr: IpAddr, port: u16) {
        self.endpoint = Some(ServiceEndpoint {
            internal_addr: addr,
            exposed_port: port,
        })
    }

    pub fn endpoint_url(&self) -> Option<Url> {
        match &self.endpoint {
            None => None,
            Some(endpoint) => Some(endpoint.to_url()),
        }
    }

    pub fn set_web_host_meta(&mut self, meta: Option<WebHostMeta>) {
        self.web_host_meta = meta;
    }

    pub fn started_at(&self) -> &DateTime<Utc> {
        &self.state.started_at
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
            name: &'a String,
            url: Option<String>,
            #[serde(rename = "type")]
            service_type: String,
            version: Option<Version>,
            open_api_url: Option<String>,
            state: &'a State,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Version {
            git_commit: Option<String>,
            software_version: Option<String>,
            date_modified: Option<DateTime<Utc>>,
        }

        let open_api_url = self.web_host_meta.clone().and_then(|meta| meta.openapi());
        let version = match &self.web_host_meta {
            Some(meta) if !meta.is_empty() => Some(Version {
                git_commit: meta.commit(),
                software_version: meta.version(),
                date_modified: meta.date_modified(),
            }),
            _ => None,
        };

        let s = Service {
            name: &self.service_name,
            url: match self.web_host_meta {
                Some(ref meta) if meta.is_valid() => self.service_url().map(|url| url.to_string()),
                _ => None,
            },
            service_type: self.container_type.to_string(),
            version,
            open_api_url,
            state: &self.state,
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
    fn parse_from_string<'de, D>(deserializer: D) -> Result<Image, D::Error>
    where
        D: Deserializer<'de>,
    {
        let img = String::deserialize(deserializer)?;
        Image::from_str(&img).map_err(de::Error::custom)
    }

    pub fn tag(&self) -> Option<String> {
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

    pub fn name(&self) -> Option<String> {
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

    pub fn registry(&self) -> Option<String> {
        match &self {
            Image::Digest { .. } => None,
            Image::Named {
                image_repository: _,
                registry,
                image_user: _,
                image_tag: _,
            } => registry.clone(),
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
#[macro_export]
macro_rules! sc {
    ( $name:expr ) => {{
        let mut hasher = Sha256::new();
        hasher.input($name);
        let img_hash = &format!("sha256:{:x}", hasher.result_reset());

        sc!($name, img_hash)
    }};

    ( $name:expr, $img:expr ) => {{
        ServiceConfig::new(String::from($name), Image::from_str($img).unwrap())
    }};

    ( $name:expr, labels = ($($l_key:expr => $l_value:expr),*),
        env = ($($env:expr),*),
        volumes = ($($v_key:expr => $v_value:expr),*) ) => {{

        let mut hasher = Sha256::new();
        hasher.input($name);
        let img_hash = &format!("sha256:{:x}", hasher.result_reset());

        let mut config =
            ServiceConfig::new(String::from($name), Image::from_str(img_hash).unwrap());

        let mut _labels = std::collections::BTreeMap::new();
        $( _labels.insert(String::from($l_key), String::from($l_value)); )*
        config.set_labels(Some(_labels));

        let mut _volumes = std::collections::BTreeMap::new();
        $( _volumes.insert(String::from($v_key), String::from($v_value)); )*
        config.set_volumes(Some(_volumes));

        let mut _env = Vec::new();
        $( _env.push(String::from($env)); )*
        config.set_env(Some(_env));

        config
    }};

    ( $name:expr, $img:expr,
        labels = ($($l_key:expr => $l_value:expr),*),
        env = ($($env:expr),*),
        volumes = ($($v_key:expr => $v_value:expr),*) ) => {{
        let mut config =
            ServiceConfig::new(String::from($name), Image::from_str($img).unwrap());

        let mut _labels = std::collections::BTreeMap::new();
        $( _labels.insert(String::from($l_key), String::from($l_value)); )*
        config.set_labels(Some(_labels));

        let mut _volumes = std::collections::BTreeMap::new();
        $( _volumes.insert(String::from($v_key), String::from($v_value)); )*
        config.set_volumes(Some(_volumes));

        let mut _env = Vec::new();
        $( _env.push(String::from($env)); )*
        config.set_env(Some(_env));

        config
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn sort_vec<T: Ord>(mut vec: Vec<T>) -> Vec<T> {
        vec.sort();
        vec
    }

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
        assert_eq!(image.name(), None);
        assert_eq!(image.tag(), None);
    }

    #[test]
    fn should_parse_image_id() {
        let image = Image::from_str("9895c9b90b58").unwrap();

        assert_eq!(&image.to_string(), "9895c9b90b58");
        assert_eq!(image.name(), None);
        assert_eq!(image.tag(), None);
    }

    #[test]
    fn should_parse_image_with_repo_and_user() {
        let image = Image::from_str("zammad/zammad-docker-compose").unwrap();

        assert_eq!(&image.name().unwrap(), "zammad/zammad-docker-compose");
        assert_eq!(&image.tag().unwrap(), "latest");
    }

    #[test]
    fn should_parse_image_with_version() {
        let image = Image::from_str("mariadb:10.3").unwrap();

        assert_eq!(&image.name().unwrap(), "library/mariadb");
        assert_eq!(&image.tag().unwrap(), "10.3");
        assert_eq!(&image.to_string(), "docker.io/library/mariadb:10.3");
    }

    #[test]
    fn should_parse_image_with_latest_version() {
        let image = Image::from_str("nginx:latest").unwrap();

        assert_eq!(&image.name().unwrap(), "library/nginx");
        assert_eq!(&image.tag().unwrap(), "latest");
        assert_eq!(&image.to_string(), "docker.io/library/nginx:latest");
    }

    #[test]
    fn should_parse_image_with_all_information() {
        let image = Image::from_str("docker.io/library/nginx:latest").unwrap();

        assert_eq!(&image.to_string(), "docker.io/library/nginx:latest");
    }

    #[test]
    fn should_parse_image_from_localhost() {
        let image = Image::from_str("localhost:5000/library/nginx:latest").unwrap();

        assert_eq!(&image.to_string(), "localhost:5000/library/nginx:latest");
        assert_eq!(&image.registry().unwrap(), "localhost:5000");
    }

    #[test]
    fn should_parse_service_config_json() {
        let json = r#"{
            "serviceName": "mariadb",
            "image": "mariadb:10.3",
            "env": [
              "MYSQL_USER=admin",
              "MYSQL_DATABASE=dbname"
            ]
          }"#;

        let config = serde_json::from_str::<ServiceConfig>(json).unwrap();

        assert_eq!(config.service_name(), "mariadb");
        assert_eq!(config.image().to_string(), "docker.io/library/mariadb:10.3");
        assert_eq!(
            config.env(),
            Some(&vec![
                String::from("MYSQL_USER=admin"),
                String::from("MYSQL_DATABASE=dbname")
            ])
        );
    }

    #[test]
    fn should_merge_service_configs_labels() {
        let mut config = sc!(
            "proxy",
            "nginx",
            labels = ("priority" => "1000", "rule" => "some_string"),
            env = (),
            volumes = ()
        );
        let config2 = sc!(
            "proxy",
            "nginx",
            labels = ("priority" => "1000", "test_label" => "other_string"),
            env = (),
            volumes = ()
        );

        config.merge_with(&config2);

        assert_eq!(config.labels().unwrap().len(), 3);
        assert_eq!(
            config.labels().unwrap().get("priority"),
            Some(&String::from("1000"))
        );
        assert_eq!(
            config.labels().unwrap().get("rule"),
            Some(&String::from("some_string"))
        );
        assert_eq!(
            config.labels().unwrap().get("test_label"),
            Some(&String::from("other_string"))
        );
    }

    #[test]
    fn should_merge_service_configs_envs() {
        let mut config = sc!(
            "proxy",
            "nginx",
            labels = (),
            env = ("VAR_1=abcd", "VAR_2=1234"),
            volumes = ()
        );

        let config2 = sc!(
            "proxy",
            "nginx",
            labels = (),
            env = ("VAR_1=efgh", "VAR_3=1234"),
            volumes = ()
        );

        config.merge_with(&config2);

        assert_eq!(config.env().unwrap().len(), 3);
        assert_eq!(
            sort_vec(config.env().unwrap().clone()),
            vec![
                String::from("VAR_1=abcd"),
                String::from("VAR_2=1234"),
                String::from("VAR_3=1234")
            ]
        );
    }

    #[test]
    fn should_merge_service_configs_volumes() {
        let mut config = sc!(
            "proxy",
            "nginx",
            labels = (),
            env = (),
            volumes = ("/etc/mysql/my.cnf" => "ABCD", "/etc/folder/abcd.conf" => "1234")
        );
        let config2 = sc!(
            "proxy",
            "nginx",
            labels = (),
            env = (),
            volumes = ("/etc/mysql/my.cnf" => "EFGH", "/etc/test.conf" => "5678")
        );

        config.merge_with(&config2);

        assert_eq!(config.volumes().unwrap().len(), 3);
        assert_eq!(
            config.volumes().unwrap().get("/etc/mysql/my.cnf"),
            Some(&String::from("ABCD"))
        );
        assert_eq!(
            config.volumes().unwrap().get("/etc/folder/abcd.conf"),
            Some(&String::from("1234"))
        );
        assert_eq!(
            config.volumes().unwrap().get("/etc/test.conf"),
            Some(&String::from("5678"))
        );
    }

    #[test]
    fn should_handle_invalid_env_when_merging_service_config() {
        let mut config = sc!(
            "proxy",
            "nginx",
            labels = (),
            env = ("abcd", "VAR=1"),
            volumes = ()
        );

        let config2 = sc!(
            "proxy",
            "nginx",
            labels = (),
            env = ("VAR=e", "invalid_env"),
            volumes = ()
        );

        config.merge_with(&config2);
        assert_eq!(
            sort_vec(config.env().unwrap().clone()),
            vec![
                String::from("VAR=1"),
                String::from("abcd"),
                String::from("invalid_env")
            ]
        );
    }
}
