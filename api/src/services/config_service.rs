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
use std::convert::{From, TryFrom};
use std::fs::File;
use std::io::prelude::*;
use std::io::Error as IOError;

use multimap::MultiMap;
use regex::Regex;
use serde::{de, Deserialize, Deserializer};
use toml::de::Error as TomlError;
use toml::from_str;

use models::service::ServiceConfig;

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
    image: String,
    env: Vec<String>,
}

#[derive(Clone, Deserialize)]
pub struct Config {
    containers: Option<ContainerConfig>,
    jira: Option<JiraConfig>,
    #[serde(deserialize_with = "Config::from_companions_table_array")]
    companions: MultiMap<String, Companion>,
}

impl Config {
    fn from_companions_table_array<'de, D>(
        deserializer: D,
    ) -> Result<MultiMap<String, Companion>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut companions = MultiMap::new();

        let companions_table_array = toml::Value::deserialize(deserializer)?;
        for companions_table in companions_table_array.as_array().unwrap_or(&vec![]) {
            if let Some(companions_table) = companions_table.as_table() {
                for (k, v) in companions_table {
                    match from_str::<Companion>(&toml::to_string(v).unwrap()) {
                        Ok(companion) => companions.insert(k.clone(), companion),
                        Err(e) => warn!("Cannot parse companion config for {}. {}", k, e),
                    };
                }
            }
        }

        Ok(companions)
    }

    pub fn load() -> Result<Config, ConfigError> {
        let mut f = match File::open("config.toml") {
            Err(e) => {
                warn!("Cannot find config file ({}) Loading default.", e);
                return Ok(Config {
                    containers: None,
                    companions: MultiMap::new(),
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
            .filter(|(k, _)| k.as_str() == "application")
            .map(|(_, v)| v)
        {
            companions.push(ServiceConfig::try_from(companion)?);
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
        let regex =
            Regex::new(r"^(((?P<registry>.+)/)?(?P<user>\w+)/)?(?P<repo>\w+)(:(?P<tag>\w+))?$")
                .unwrap();
        let captures = match regex.captures(&companion.image) {
            Some(captures) => captures,
            None => return Err(ConfigError::UnableToParseImage),
        };

        let repo = captures
            .name("repo")
            .map(|m| String::from(m.as_str()))
            .unwrap();
        let registry = captures.name("registry").map(|m| String::from(m.as_str()));
        let user = captures.name("user").map(|m| String::from(m.as_str()));
        let tag = captures.name("tag").map(|m| String::from(m.as_str()));

        let mut config =
            ServiceConfig::new(&companion.service_name, &repo, Some(companion.env.clone()));
        config.set_registry(&registry);
        config.set_image_user(&user);
        config.set_image_tag(&tag);

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
            [companions.application]
            serviceName = 'openid'
            image = 'private.example.com/library/opendid:latest'
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
        });
    }

    #[test]
    fn should_return_application_companions_as_error_when_invalid_image_is_provided() {
        let config_str = r#"
            [[companions]]
            [companions.application]
            serviceName = 'openid'
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
