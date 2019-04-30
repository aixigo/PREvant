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

use crate::models::service::{ContainerType, Image, ServiceConfig};
use base64::decode;
use regex::Regex;
use secstr::SecUtf8;
use serde::{de, Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::convert::{From, TryFrom};
use std::fs::File;
use std::io::prelude::*;
use std::io::Error as IOError;
use std::str::FromStr;
use toml::de::Error as TomlError;
use toml::from_str;

fn default_app_selector() -> Regex {
    Regex::new(".+").unwrap()
}

#[derive(Clone, Deserialize)]
pub struct ContainerConfig {
    #[serde(deserialize_with = "ContainerConfig::parse_from_memory_string")]
    memory_limit: Option<u64>,
}

#[derive(Clone, Deserialize)]
pub struct JiraConfig {
    host: String,
    user: String,
    #[serde(deserialize_with = "JiraConfig::parse_secstr")]
    password: SecUtf8,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Companion {
    service_name: String,
    #[serde(rename = "type")]
    companion_type: CompanionType,
    image: String,
    env: Option<Vec<String>>,
    labels: Option<BTreeMap<String, String>>,
    volumes: Option<BTreeMap<String, String>>,
    #[serde(with = "serde_regex", default = "default_app_selector")]
    app_selector: Regex,
}

#[derive(Clone, Deserialize, PartialEq)]
enum CompanionType {
    #[serde(rename = "application")]
    Application,
    #[serde(rename = "service")]
    Service,
}

#[derive(Clone, Deserialize)]
struct Service {
    secrets: Option<Vec<Secret>>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Secret {
    name: String,
    #[serde(deserialize_with = "Secret::parse_secstr", rename = "data")]
    secret: SecUtf8,
    #[serde(with = "serde_regex", default = "default_app_selector")]
    app_selector: Regex,
}

#[derive(Clone, Default, Deserialize)]
pub struct Config {
    containers: Option<ContainerConfig>,
    jira: Option<JiraConfig>,
    companions: Option<BTreeMap<String, Companion>>,
    services: Option<BTreeMap<String, Service>>,
}

impl Config {
    pub fn load(path: &str) -> Result<Config, ConfigError> {
        let mut f = File::open(path)?;

        let mut contents = String::new();
        f.read_to_string(&mut contents)?;

        let config = from_str::<Config>(contents.as_str())?;
        Ok(config)
    }

    pub fn get_container_config(&self) -> ContainerConfig {
        match &self.containers {
            Some(containers) => containers.clone(),
            None => ContainerConfig { memory_limit: None },
        }
    }

    pub fn get_jira_config(&self) -> Option<JiraConfig> {
        match &self.jira {
            None => None,
            Some(j) => Some(j.clone()),
        }
    }

    pub fn get_service_companion_configs(
        &self,
        app_name: &str,
    ) -> Result<Vec<ServiceConfig>, ConfigError> {
        Ok(self.get_companion_configs(app_name, |companion| {
            companion.companion_type == CompanionType::Service
        })?)
    }

    pub fn get_application_companion_configs(
        &self,
        app_name: &str,
    ) -> Result<Vec<ServiceConfig>, ConfigError> {
        Ok(self.get_companion_configs(app_name, |companion| {
            companion.companion_type == CompanionType::Application
        })?)
    }

    fn get_companion_configs<P>(
        &self,
        app_name: &str,
        predicate: P,
    ) -> Result<Vec<ServiceConfig>, ConfigError>
    where
        P: Fn(&Companion) -> bool,
    {
        match &self.companions {
            None => Ok(vec![]),
            Some(companions_map) => {
                let mut companions = Vec::new();

                for (_, companion) in companions_map
                    .iter()
                    .filter(|(_, companion)| companion.app_selector.is_match(app_name))
                    .filter(|(_, companion)| predicate(*companion))
                {
                    let mut config = ServiceConfig::try_from(companion)?;
                    config.set_container_type(ContainerType::from(&companion.companion_type));

                    companions.push(config);
                }

                Ok(companions)
            }
        }
    }

    pub fn add_secrets_to(&self, service_config: &mut ServiceConfig, app_name: &str) {
        if let Some(services) = &self.services {
            if let Some(service) = services.get(service_config.service_name()) {
                service.add_secrets_to(service_config, app_name);
            }
        }
    }
}

impl JiraConfig {
    pub fn host(&self) -> &String {
        &self.host
    }
    pub fn user(&self) -> &String {
        &self.user
    }
    pub fn password(&self) -> &SecUtf8 {
        &self.password
    }

    fn parse_secstr<'de, D>(deserializer: D) -> Result<SecUtf8, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secret = String::deserialize(deserializer)?;
        Ok(SecUtf8::from(secret))
    }
}

impl ContainerConfig {
    fn parse_from_memory_string<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let container_limit = String::deserialize(deserializer)?;

        let (size, unit) = container_limit.split_at(container_limit.len() - 1);
        let limit = size.parse::<u64>().map_err(de::Error::custom)?;

        let exp = match unit.to_lowercase().as_str() {
            "k" => 1,
            "m" => 2,
            "g" => 3,
            _ => 0,
        };

        Ok(Some(limit * 1024_u64.pow(exp)))
    }

    pub fn get_memory_limit(&self) -> Option<u64> {
        match self.memory_limit {
            None => None,
            Some(limit) => Some(limit.clone()),
        }
    }
}

impl Service {
    pub fn add_secrets_to(&self, service_config: &mut ServiceConfig, app_name: &str) {
        if let Some(secrets) = &self.secrets {
            for s in secrets.iter().filter(|s| s.matches(app_name)) {
                service_config.add_volume(
                    format!("/run/secrets/{}", s.name),
                    // TODO: use secstr in service_config (see issue #8)
                    String::from(s.secret.unsecure()),
                );
            }
        }
    }
}

impl Secret {
    pub fn matches(&self, app_name: &str) -> bool {
        self.app_selector.is_match(app_name)
    }

    fn parse_secstr<'de, D>(deserializer: D) -> Result<SecUtf8, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secret = String::deserialize(deserializer)?;
        let decoded = decode(&secret).map_err(de::Error::custom)?;
        Ok(SecUtf8::from(decoded))
    }
}

#[derive(Debug, Fail)]
pub enum ConfigError {
    #[fail(display = "Cannot open config file. {}", error)]
    CannotOpenConfigFile { error: IOError },
    #[fail(display = "Invalid config file format. {}", error)]
    ConfigFormatError { error: TomlError },
    #[fail(display = "Unable to parse image string {}.", image)]
    UnableToParseImage { image: String },
}

impl From<IOError> for ConfigError {
    fn from(error: IOError) -> Self {
        ConfigError::CannotOpenConfigFile { error }
    }
}

impl From<TomlError> for ConfigError {
    fn from(error: TomlError) -> Self {
        ConfigError::ConfigFormatError { error }
    }
}

impl TryFrom<&Companion> for ServiceConfig {
    type Error = ConfigError;

    fn try_from(companion: &Companion) -> Result<ServiceConfig, ConfigError> {
        let image = match Image::from_str(&companion.image) {
            Ok(image) => image,
            Err(_) => {
                return Err(ConfigError::UnableToParseImage {
                    image: companion.image.clone(),
                })
            }
        };

        let mut config = ServiceConfig::new(companion.service_name.clone(), image);
        config.set_env(companion.env.clone());
        config.set_labels(companion.labels.clone());

        if let Some(volumes) = &companion.volumes {
            config.set_volumes(Some(volumes.clone()));
        }

        Ok(config)
    }
}

impl From<&CompanionType> for ContainerType {
    fn from(t: &CompanionType) -> Self {
        match t {
            CompanionType::Application => ContainerType::ApplicationCompanion,
            CompanionType::Service => ContainerType::ServiceCompanion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    macro_rules! service_config {
        ( $name:expr ) => {{
            let mut hasher = Sha256::new();
            hasher.input($name);
            let img_hash = &format!("sha256:{:x}", hasher.result_reset());

            ServiceConfig::new(String::from($name), Image::from_str(&img_hash).unwrap())
        }};
    }

    macro_rules! config_from_str {
        ( $config_str:expr ) => {
            from_str::<Config>($config_str).unwrap()
        };
    }

    #[test]
    fn should_return_application_companions_as_service_configs() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ 'KEY=VALUE' ]

            [companions.nginx]
            serviceName = '{{service-name}}-nginx'
            type = 'service'
            image = 'nginx:latest'
            env = [ 'KEY=VALUE' ]
            "#
        );

        let companion_configs = config.get_application_companion_configs("master").unwrap();

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            assert_eq!(config.service_name(), "openid");
            assert_eq!(
                &config.image().to_string(),
                "private.example.com/library/openid:latest"
            );
            assert_eq!(
                config.container_type(),
                &ContainerType::ApplicationCompanion
            );
            assert_eq!(config.labels(), None);
        });
    }

    #[test]
    fn should_return_service_companions_as_service_configs() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ 'KEY=VALUE' ]

            [companions.nginx]
            serviceName = '{{service-name}}-nginx'
            type = 'service'
            image = 'nginx:latest'
            env = [ 'KEY=VALUE' ]
            "#
        );

        let companion_configs = config.get_service_companion_configs("master").unwrap();

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            assert_eq!(config.service_name(), "{{service-name}}-nginx");
            assert_eq!(
                &config.image().to_string(),
                "docker.io/library/nginx:latest"
            );
            assert_eq!(config.container_type(), &ContainerType::ServiceCompanion);
            assert_eq!(config.labels(), None);
        });
    }

    #[test]
    fn should_return_application_companions_as_service_configs_with_volumes() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:11-alpine'
            env = [ 'KEY=VALUE' ]

            [companions.openid.volumes]
            '/tmp/test-1.json' = '{}'
            '/tmp/test-2.json' = '{}'
            "#
        );

        let companion_configs = config.get_application_companion_configs("master").unwrap();

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            assert_eq!(config.volumes().unwrap().len(), 2);
        });
    }

    #[test]
    fn should_return_application_companions_as_service_configs_with_labels() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:11-alpine'

            [companions.openid.labels]
            'com.example.foo' = 'bar'
            "#
        );

        let companion_configs = config.get_application_companion_configs("master").unwrap();

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            for (k, v) in config.labels().unwrap().iter() {
                assert_eq!(k, "com.example.foo");
                assert_eq!(v, "bar");
            }
        });
    }

    #[test]
    fn should_return_application_companions_as_error_when_invalid_image_is_provided() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = ''
            env = [ 'KEY=VALUE' ]
            "#
        );

        let result = config.get_application_companion_configs("master");

        assert_eq!(result.is_err(), true);
        match result.err().unwrap() {
            ConfigError::UnableToParseImage { image: _ } => assert!(true),
            _ => assert!(false, "unexpected error"),
        }
    }

    #[test]
    fn should_return_application_companions_with_specific_app_selector() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ 'KEY=VALUE' ]
            appSelector = "master"
            "#
        );

        let companion_configs = config.get_application_companion_configs("master").unwrap();

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            assert_eq!(config.service_name(), "openid");
            assert_eq!(
                &config.image().to_string(),
                "private.example.com/library/openid:latest"
            );
            assert_eq!(
                config.container_type(),
                &ContainerType::ApplicationCompanion
            );
            assert_eq!(config.labels(), None);
        });
    }

    #[test]
    fn should_not_return_application_companions_with_specific_app_selector() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ 'KEY=VALUE' ]
            appSelector = "master"
            "#
        );

        let companion_configs = config
            .get_application_companion_configs("random-name")
            .unwrap();

        assert_eq!(companion_configs.len(), 0);
    }

    #[test]
    fn should_set_service_secrets_with_default_app_selector() {
        let config = config_from_str!(
            r#"
            [services.mariadb]
            [[services.mariadb.secrets]]
            name = "user"
            data = "SGVsbG8="
            "#
        );

        let mut service_config = service_config!("mariadb");
        config.add_secrets_to(&mut service_config, &String::from("master"));

        let secret_file_content = service_config
            .volumes()
            .expect("File content is missing")
            .get("/run/secrets/user")
            .expect("No file for /run/secrets/user");
        assert_eq!(secret_file_content, "Hello");
    }

    #[test]
    fn should_set_service_secrets_with_specific_app_selector() {
        let config = config_from_str!(
            r#"
            [services.mariadb]
            [[services.mariadb.secrets]]
            name = "user"
            data = "SGVsbG8="
            appSelector = "master"
            "#
        );

        let mut service_config = service_config!("mariadb");
        config.add_secrets_to(&mut service_config, &String::from("master"));

        let secret_file_content = service_config
            .volumes()
            .expect("File content is missing")
            .get("/run/secrets/user")
            .expect("No file for /run/secrets/user");
        assert_eq!(secret_file_content, "Hello");
    }

    #[test]
    fn should_not_set_service_secrets_with_specific_app_selector() {
        let config = config_from_str!(
            r#"
            [services.mariadb]
            [[services.mariadb.secrets]]
            name = "user"
            data = "SGVsbG8="
            appSelector = "master-.+"
            "#
        );

        let mut service_config = service_config!("mariadb");
        config.add_secrets_to(&mut service_config, &String::from("random-app-name"));

        assert!(service_config.volumes().is_none());
    }

    #[test]
    fn should_not_parse_config_because_of_invalid_secret_data() {
        let config_str = r#"
            [services.mariadb]
            [[services.mariadb.secrets]]
            name = "user"
            data = "+++"
        "#;

        let config_result = from_str::<Config>(config_str);
        assert!(config_result.is_err(), "should not parse config");
    }
}
