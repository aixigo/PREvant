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
use std::convert::From;
use std::fs::File;
use std::io::prelude::*;
use std::io::Error as IOError;

use models::service::ServiceConfig;
use multimap::MultiMap;
use regex::Regex;
use serde::{de, Deserialize, Deserializer};
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
                    let companion = from_str::<Companion>(&toml::to_string(v).unwrap()).unwrap();
                    companions.insert(k.clone(), companion);
                }
            }
        }

        Ok(companions)
    }

    pub fn load() -> Result<Config, ConfigError> {
        let mut f = File::open("config.toml")?;

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

    pub fn get_application_companion_configs(&self) -> Vec<ServiceConfig> {
        self.companions
            .iter()
            .filter(|(k, _)| k.as_str() == "application")
            .map(|(_, v)| v)
            .map(|companion| ServiceConfig::from(companion))
            .collect()
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

impl From<&Companion> for ServiceConfig {
    fn from(companion: &Companion) -> Self {
        let regex =
            Regex::new(r"^(((?P<registry>.+)/)?(?P<user>\w+)/)?(?P<repo>\w+)(:(?P<tag>\w+))?$")
                .unwrap();
        let captures = regex.captures(&companion.image).unwrap();

        let repo = captures
            .name("repo")
            .map(|m| String::from(m.as_str()))
            .unwrap();
        let registry = captures.name("registry").map(|m| String::from(m.as_str()));
        let user = captures.name("user").map(|m| String::from(m.as_str()));
        let tag = captures.name("tag").map(|m| String::from(m.as_str()));

        let mut config = ServiceConfig::new(&companion.service_name, &repo, None);
        config.set_registry(&registry);
        config.set_image_user(&user);
        config.set_image_tag(&tag);

        config
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
        let companion_configs = config.get_application_companion_configs();

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
}
