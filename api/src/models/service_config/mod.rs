/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2020 aixigo AG
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
use crate::config::Routing;
use crate::models::{ContainerType, Image};
pub use environment::{Environment, EnvironmentVariable};
use secstr::SecUtf8;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

mod environment;
mod templating;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    service_name: String,
    image: Image,
    env: Option<Environment>,
    #[serde(alias = "volumes", alias = "files", default)]
    files: Option<BTreeMap<PathBuf, SecUtf8>>,
    #[serde(skip)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(skip, default = "ContainerType::default")]
    container_type: ContainerType,
    #[serde(skip)]
    port: u16,
    #[serde(skip)]
    routing: Option<Routing>,
}

impl ServiceConfig {
    pub fn new(service_name: String, image: Image) -> ServiceConfig {
        ServiceConfig {
            service_name,
            image,
            env: None,
            files: None,
            labels: None,
            container_type: ContainerType::Instance,
            port: 80,
            routing: None,
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

    pub fn set_env(&mut self, env: Option<Environment>) {
        self.env = env;
    }

    pub fn env<'a, 'b: 'a>(&'b self) -> Option<&'a Environment> {
        match &self.env {
            None => None,
            Some(env) => Some(env),
        }
    }

    #[deprecated]
    pub fn set_labels(&mut self, labels: Option<BTreeMap<String, String>>) {
        self.labels = labels;
    }

    #[deprecated]
    pub fn labels<'a, 'b: 'a>(&'b self) -> Option<&'a BTreeMap<String, String>> {
        match &self.labels {
            None => None,
            Some(labels) => Some(labels),
        }
    }

    pub fn add_file(&mut self, path: PathBuf, data: SecUtf8) {
        if let Some(ref mut files) = self.files {
            files.insert(path, data);
        } else {
            let mut files = BTreeMap::new();
            files.insert(path, data);
            self.files = Some(files);
        }
    }

    pub fn set_files(&mut self, files: Option<BTreeMap<PathBuf, SecUtf8>>) {
        self.files = files
    }

    pub fn files<'a, 'b: 'a>(&'b self) -> Option<&'a BTreeMap<PathBuf, SecUtf8>> {
        match &self.files {
            None => None,
            Some(files) => Some(files),
        }
    }

    pub fn set_port(&mut self, port: u16) {
        self.port = port;
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn set_routing(&mut self, routing: Routing) {
        self.routing = Some(routing);
    }

    pub fn routing<'a, 'b: 'a>(&'b self) -> Option<&'a Routing> {
        self.routing.as_ref()
    }

    /// Copy labels, envs and files from other into self.
    /// If something is defined in self and other, self has precedence.
    pub fn merge_with(&mut self, other: &Self) {
        if let Some(env) = &other.env {
            self.env = match self.env.take() {
                Some(mut self_env) => {
                    for env in env.iter() {
                        if self_env.variable(env.key()).is_some() {
                            continue;
                        }
                        self_env.push(env.clone());
                    }
                    Some(self_env)
                }
                None => Some(env.clone()),
            }
        }

        let mut files = other.files.as_ref().cloned().unwrap_or_default();
        files.extend(self.files.as_ref().cloned().unwrap_or_default());
        self.files = Some(files);

        let mut labels = other.labels.as_ref().cloned().unwrap_or_default();
        labels.extend(self.labels.as_ref().cloned().unwrap_or_default());
        self.labels = Some(labels);
    }
}

#[cfg(test)]
#[macro_export]
macro_rules! sc {
    ( $name:expr ) => {{
        use sha2::Digest;
        let mut hasher = ::sha2::Sha256::new();
        hasher.update($name);
        let img_hash = &format!("sha256:{:x}", hasher.finalize());

        $crate::sc!($name, img_hash)
    }};

    ( $name:expr, $img:expr ) => {{
        use std::str::FromStr;
        use $crate::models::ServiceConfig;
        ServiceConfig::new(String::from($name), crate::models::Image::from_str($img).unwrap())
    }};

    ( $name:expr, labels = ($($l_key:expr => $l_value:expr),*),
        env = ($($env_key:expr => $env_value:expr),*),
        files = ($($v_key:expr => $v_value:expr),*) ) => {{
        use std::str::FromStr;
        use sha2::Digest;

        let mut hasher = ::sha2::Sha256::new();
        hasher.update($name);
        let img_hash = &format!("sha256:{:x}", hasher.finalize());

        let mut config =
            ServiceConfig::new(String::from($name), crate::models::Image::from_str(img_hash).unwrap());

        let mut _labels = std::collections::BTreeMap::new();
        $( _labels.insert(String::from($l_key), String::from($l_value)); )*
        config.set_labels(Some(_labels));

        let mut _files = std::collections::BTreeMap::new();
        $( _files.insert(std::path::PathBuf::from($v_key), String::from($v_value)); )*
        config.set_files(Some(_files));

        let mut _env = Vec::new();
        $( _env.push(crate::models::EnvironmentVariable::new(String::from($env_key), secstr::SecUtf8::from($env_value))); )*
        config.set_env(Some(crate::models::Environment::new(_env)));

        config
    }};

    ( $name:expr, $img:expr,
        env = ($($env_key:expr => $env_value:expr),*)
        ) => {{
        use std::str::FromStr;
        let mut config =
            ServiceConfig::new(String::from($name), crate::models::Image::from_str($img).unwrap());

        let mut _env = Vec::new();
        $( _env.push(crate::models::EnvironmentVariable::new(String::from($env_key), secstr::SecUtf8::from($env_value))); )*
        config.set_env(Some(crate::models::Environment::new(_env)));

        config
    }};

    ( $name:expr, $img:expr,
        labels = ($($l_key:expr => $l_value:expr),*),
        env = ($($env_key:expr => $env_value:expr),*),
        files = ($($v_key:expr => $v_value:expr),*) ) => {{
        use std::str::FromStr;
        let mut config =
            ServiceConfig::new(String::from($name), crate::models::Image::from_str($img).unwrap());

        let mut _labels = std::collections::BTreeMap::new();
        $( _labels.insert(String::from($l_key), String::from($l_value)); )*
        config.set_labels(Some(_labels));

        let mut _files = std::collections::BTreeMap::new();
        $( _files.insert(std::path::PathBuf::from($v_key), SecUtf8::from($v_value)); )*
        config.set_files(Some(_files));

        let mut _env = Vec::new();
        $( _env.push(crate::models::EnvironmentVariable::new(String::from($env_key), secstr::SecUtf8::from($env_value))); )*
        config.set_env(Some(crate::models::Environment::new(_env)));

        config
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::from_value;

    #[test]
    fn should_parse_service_config_json() {
        let config = from_value::<ServiceConfig>(serde_json::json!({
            "serviceName": "mariadb",
            "image": "mariadb:10.3",
            "env": [
              "MYSQL_USER=admin",
              "MYSQL_DATABASE=dbname"
            ]
        }))
        .unwrap();

        assert_eq!(config.service_name(), "mariadb");
        assert_eq!(config.image().to_string(), "docker.io/library/mariadb:10.3");
        assert_eq!(
            config.env(),
            Some(&Environment::new(vec![
                EnvironmentVariable::new("MYSQL_USER".to_string(), SecUtf8::from("admin")),
                EnvironmentVariable::new("MYSQL_DATABASE".to_string(), SecUtf8::from("dbname"))
            ]))
        );
    }

    #[test]
    fn should_merge_service_configs_labels() {
        let mut config = sc!(
            "proxy",
            "nginx",
            labels = ("priority" => "1000", "rule" => "some_string"),
            env = (),
            files = ()
        );
        let config2 = sc!(
            "proxy",
            "nginx",
            labels = ("priority" => "2000", "test_label" => "other_string"),
            env = (),
            files = ()
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
            env = ("VAR_1" => "abcd", "VAR_2" => "1234")
        );

        let config2 = sc!(
            "proxy",
            "nginx",
            env = ("VAR_1" => "efgh", "VAR_3" => "1234")
        );

        config.merge_with(&config2);

        let env = config.env().unwrap();
        assert_eq!(env.iter().count(), 3);
        assert_eq!(
            env.variable("VAR_1"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_1"),
                SecUtf8::from("abcd")
            ))
        );
        assert_eq!(
            env.variable("VAR_2"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_2"),
                SecUtf8::from("1234")
            ))
        );
        assert_eq!(
            env.variable("VAR_3"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_3"),
                SecUtf8::from("1234")
            ))
        );
    }

    #[test]
    fn should_merge_service_configs_volumes() {
        let mut config = sc!(
            "proxy",
            "nginx",
            labels = (),
            env = (),
            files = ("/etc/mysql/my.cnf" => "ABCD", "/etc/folder/abcd.conf" => "1234")
        );
        let config2 = sc!(
            "proxy",
            "nginx",
            labels = (),
            env = (),
            files = ("/etc/mysql/my.cnf" => "EFGH", "/etc/test.conf" => "5678")
        );

        config.merge_with(&config2);

        assert_eq!(config.files().expect("No value found").len(), 3);
        assert_eq!(
            config
                .files()
                .unwrap()
                .get(&PathBuf::from("/etc/mysql/my.cnf")),
            Some(&SecUtf8::from("ABCD"))
        );
        assert_eq!(
            config
                .files()
                .unwrap()
                .get(&PathBuf::from("/etc/folder/abcd.conf")),
            Some(&SecUtf8::from("1234"))
        );
        assert_eq!(
            config
                .files()
                .unwrap()
                .get(&PathBuf::from("/etc/test.conf")),
            Some(&SecUtf8::from("5678"))
        );
    }

    #[test]
    fn should_parse_volume_service_config_json() {
        let config_string = r#"{
            "serviceName": "mariadb",
            "image": "mariadb:10.3",
            "env": [
              "MYSQL_USER=admin",
              "MYSQL_DATABASE=dbname"
            ],
            "volumes": {
                "/etc/mysql/my.cnf": "ABCD"
            }
        }"#;
        let config_volume =
            from_value::<ServiceConfig>(serde_json::from_slice(config_string.as_bytes()).unwrap())
                .unwrap();

        let config_file = from_value::<ServiceConfig>(serde_json::json!({
            "serviceName": "mariadb",
            "image": "mariadb:10.3",
            "env": [
              "MYSQL_USER=admin",
              "MYSQL_DATABASE=dbname"
            ],
            "files": {
                "/etc/mysql/my.cnf" : "EFGH"
            }
        }))
        .unwrap();

        assert_eq!(
            config_volume
                .files()
                .unwrap()
                .get(&PathBuf::from("/etc/mysql/my.cnf")),
            Some(&SecUtf8::from("ABCD"))
        );

        assert_eq!(
            config_file
                .files()
                .unwrap()
                .get(&PathBuf::from("/etc/mysql/my.cnf")),
            Some(&SecUtf8::from("EFGH"))
        );
    }
}
