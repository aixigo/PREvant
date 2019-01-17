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
use crate::models::service::{parse_image_string, ContainerType, ServiceConfig};
use serde::{de, Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::convert::{From, TryFrom};
use std::fs::File;
use std::io::prelude::*;
use std::io::Error as IOError;
use toml::de::Error as TomlError;
use toml::from_str;

#[derive(Clone, Deserialize)]
pub struct ContainerConfig {
    #[serde(deserialize_with = "ContainerConfig::parse_from_memory_string")]
    memory_limit: Option<u64>,
}

#[derive(Clone, Deserialize)]
pub struct JiraConfig {
    host: String,
    user: String,
    password: String,
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
}

#[derive(Clone, Deserialize, PartialEq)]
enum CompanionType {
    #[serde(rename = "application")]
    Application,
    #[serde(rename = "service")]
    Service,
}

#[derive(Clone, Deserialize)]
pub struct Config {
    containers: Option<ContainerConfig>,
    jira: Option<JiraConfig>,
    companions: Vec<BTreeMap<String, Companion>>,
}

impl Config {
    pub fn load() -> Result<Config, ConfigError> {
        let mut f = match File::open("config.toml") {
            Err(e) => {
                warn!("Cannot find config file ({}) Loading default.", e);
                return Ok(Config {
                    containers: None,
                    companions: Vec::new(),
                    jira: None,
                });
            }
            Ok(f) => f,
        };

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

    pub fn get_application_companion_configs(&self) -> Result<Vec<ServiceConfig>, ConfigError> {
        let mut companions = Vec::new();

        for companion in self
            .companions
            .iter()
            .flat_map(|companions| companions.values())
            .filter(|companion| companion.companion_type == CompanionType::Application)
        {
            let mut config = ServiceConfig::try_from(companion)?;

            config.set_container_type(match &companion.companion_type {
                CompanionType::Application => ContainerType::ApplicationCompanion,
                CompanionType::Service => ContainerType::ServiceCompanion,
            });

            companions.push(config);
        }

        Ok(companions)
    }
}

impl JiraConfig {
    pub fn get_host(&self) -> String {
        self.host.clone()
    }
    pub fn get_user(&self) -> String {
        self.user.clone()
    }
    pub fn get_password(&self) -> String {
        self.password.clone()
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

#[derive(Debug)]
pub enum ConfigError {
    CannotOpenConfigFile(IOError),
    ConfigFormatError(TomlError),
    UnableToParseImage,
}

impl From<IOError> for ConfigError {
    fn from(err: IOError) -> Self {
        ConfigError::CannotOpenConfigFile(err)
    }
}

impl From<TomlError> for ConfigError {
    fn from(err: TomlError) -> Self {
        ConfigError::ConfigFormatError(err)
    }
}

impl TryFrom<&Companion> for ServiceConfig {
    type Error = ConfigError;

    fn try_from(companion: &Companion) -> Result<ServiceConfig, ConfigError> {
        let (repo, user, registry, tag) = match parse_image_string(&companion.image) {
            Ok((repo, user, registry, tag)) => (repo, user, registry, tag),
            Err(_) => return Err(ConfigError::UnableToParseImage),
        };

        let mut config = ServiceConfig::new(companion.service_name.clone(), repo);
        config.set_registry(registry);
        config.set_image_user(user);
        config.set_image_tag(tag);
        config.set_env(companion.env.clone());
        config.set_labels(companion.labels.clone());

        if let Some(volumes) = &companion.volumes {
            config.set_volumes(Some(volumes.clone()));
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_return_application_companions_as_service_configs() {
        let config_str = r#"
            [[companions]]
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/opendid:latest'
            env = [ 'KEY=VALUE' ]

            [companions.nginx]
            serviceName = '{{service-name}}-nginx'
            type = 'service'
            image = 'nginx:latest'
            env = [ 'KEY=VALUE' ]
        "#;

        let config = from_str::<Config>(config_str).unwrap();
        let companion_configs = config.get_application_companion_configs().unwrap();

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            assert_eq!(config.get_service_name(), "openid");
            assert_eq!(
                config.get_docker_image(),
                "private.example.com/library/opendid:latest"
            );
            assert_eq!(config.get_image_tag(), "latest");
            assert_eq!(
                config.get_container_type(),
                &ContainerType::ApplicationCompanion
            );
            assert_eq!(config.get_labels(), None);
        });
    }

    #[test]
    fn should_return_application_companions_as_service_configs_with_volumes() {
        let config_str = r#"
            [[companions]]
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/opendid:11-alpine'
            env = [ 'KEY=VALUE' ]

            [companions.openid.volumes]
            '/tmp/test-1.json' = '{}'
            '/tmp/test-2.json' = '{}'
        "#;

        let config = from_str::<Config>(config_str).unwrap();
        let companion_configs = config.get_application_companion_configs().unwrap();

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            assert_eq!(config.get_volumes().unwrap().len(), 2);
        });
    }

    #[test]
    fn should_return_application_companions_as_service_configs_with_labels() {
        let config_str = r#"
            [[companions]]
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/opendid:11-alpine'

            [companions.openid.labels]
            'com.example.foo' = 'bar'
        "#;

        let config = from_str::<Config>(config_str).unwrap();
        let companion_configs = config.get_application_companion_configs().unwrap();

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            for (k, v) in config.get_labels().unwrap().iter() {
                assert_eq!(k, "com.example.foo");
                assert_eq!(v, "bar");
            }
        });
    }

    #[test]
    fn should_return_application_companions_as_error_when_invalid_image_is_provided() {
        let config_str = r#"
            [[companions]]
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = ''
            env = [ 'KEY=VALUE' ]
        "#;

        let config = from_str::<Config>(config_str).unwrap();
        let result = config.get_application_companion_configs();

        assert_eq!(result.is_err(), true);
        match result.err().unwrap() {
            ConfigError::UnableToParseImage => assert!(true),
            _ => assert!(false, "unexpected error"),
        }
    }
}
