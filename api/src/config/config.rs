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
use crate::config::{Companion, CompanionType, ContainerConfig, Runtime, Secret};
use crate::models::ServiceConfig;
use secstr::SecUtf8;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::convert::From;
use std::fs::File;
use std::io::prelude::*;
use std::io::Error as IOError;
use std::path::PathBuf;
use toml::de::Error as TomlError;
use toml::from_str;

#[derive(Clone, Deserialize)]
pub struct JiraConfig {
    host: String,
    user: String,
    password: SecUtf8,
}

#[derive(Clone, Deserialize)]
struct Service {
    secrets: Option<Vec<Secret>>,
}

#[derive(Clone, Default, Deserialize)]
pub struct Config {
    runtime: Option<Runtime>,
    containers: Option<ContainerConfig>,
    jira: Option<JiraConfig>,
    companions: Option<BTreeMap<String, Companion>>,
    services: Option<BTreeMap<String, Service>>,
    hooks: Option<BTreeMap<String, PathBuf>>,
}

impl Config {
    pub fn load(path: &str) -> Result<Config, ConfigError> {
        let mut f = File::open(path)?;

        let mut contents = String::new();
        f.read_to_string(&mut contents)?;

        let config = from_str::<Config>(contents.as_str())?;
        Ok(config)
    }

    pub fn runtime_config(&self) -> Runtime {
        match &self.runtime {
            Some(runtime) => runtime.clone(),
            None => Runtime::default(),
        }
    }

    pub fn container_config(&self) -> ContainerConfig {
        match &self.containers {
            Some(containers) => containers.clone(),
            None => ContainerConfig::default(),
        }
    }

    pub fn jira_config(&self) -> Option<JiraConfig> {
        match &self.jira {
            None => None,
            Some(j) => Some(j.clone()),
        }
    }

    pub fn service_companion_configs(&self, app_name: &str) -> Vec<ServiceConfig> {
        self.companion_configs(app_name, |companion| {
            companion.companion_type() == &CompanionType::Service
        })
    }

    pub fn application_companion_configs(&self, app_name: &str) -> Vec<ServiceConfig> {
        self.companion_configs(app_name, |companion| {
            companion.companion_type() == &CompanionType::Application
        })
    }

    fn companion_configs<P>(&self, app_name: &str, predicate: P) -> Vec<ServiceConfig>
    where
        P: Fn(&Companion) -> bool,
    {
        match &self.companions {
            None => vec![],
            Some(companions_map) => companions_map
                .iter()
                .filter(|(_, companion)| companion.matches_app_name(app_name))
                .filter(|(_, companion)| predicate(*companion))
                .map(|(_, companion)| companion.clone().into())
                .collect(),
        }
    }

    pub fn add_secrets_to(&self, service_config: &mut ServiceConfig, app_name: &str) {
        if let Some(services) = &self.services {
            if let Some(service) = services.get(service_config.service_name()) {
                service.add_secrets_to(service_config, app_name);
            }
        }
    }

    pub fn hook(&self, hook_name: &str) -> Option<&PathBuf> {
        self.hooks
            .as_ref()
            .map(|hooks| hooks.get(hook_name))
            .flatten()
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
}

impl Service {
    pub fn add_secrets_to(&self, service_config: &mut ServiceConfig, app_name: &str) {
        if let Some(secrets) = &self.secrets {
            for s in secrets.iter().filter(|s| s.matches_app_name(app_name)) {
                let (path, sec) = s.clone().into();

                service_config.add_volume(
                    path,
                    // TODO: use secstr in service_config (see issue #8)
                    String::from(sec.unsecure()),
                );
            }
        }
    }
}

#[derive(Debug, Fail)]
pub enum ConfigError {
    #[fail(display = "Cannot open config file. {}", error)]
    CannotOpenConfigFile { error: IOError },
    #[fail(display = "Invalid config file format. {}", error)]
    ConfigFormatError { error: TomlError },
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

#[cfg(test)]
#[macro_export]
macro_rules! config_from_str {
    ( $config_str:expr ) => {
        toml::from_str::<Config>($config_str).unwrap()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{service::ContainerType, Image};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use std::str::FromStr;

    macro_rules! service_config {
        ( $name:expr ) => {{
            let mut hasher = Sha256::new();
            hasher.input($name);
            let img_hash = &format!("sha256:{:x}", hasher.result_reset());

            ServiceConfig::new(String::from($name), Image::from_str(&img_hash).unwrap())
        }};
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

        let companion_configs = config.application_companion_configs("master");

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

        let companion_configs = config.service_companion_configs("master");

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

        let companion_configs = config.application_companion_configs("master");

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

        let companion_configs = config.application_companion_configs("master");

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|config| {
            for (k, v) in config.labels().unwrap().iter() {
                assert_eq!(k, "com.example.foo");
                assert_eq!(v, "bar");
            }
        });
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

        let companion_configs = config.application_companion_configs("master");

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

        let companion_configs = config.application_companion_configs("random-name");

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
            .get(&PathBuf::from("/run/secrets/user"))
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
            .get(&PathBuf::from("/run/secrets/user"))
            .expect("No file for /run/secrets/user");
        assert_eq!(secret_file_content, "Hello");
    }

    #[test]
    fn should_set_service_secrets_with_regex_app_selector() {
        let config = config_from_str!(
            r#"
            [services.mariadb]
            [[services.mariadb.secrets]]
            name = "user"
            data = "SGVsbG8="
            appSelector = "master(-.+)?"
            "#
        );

        let mut service_config = service_config!("mariadb");
        config.add_secrets_to(&mut service_config, &String::from("master-1.x"));

        let secret_file_content = service_config
            .volumes()
            .expect("File content is missing")
            .get(&PathBuf::from("/run/secrets/user"))
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
    fn should_not_set_service_secrets_with_partially_specific_app_selector() {
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
        config.add_secrets_to(&mut service_config, &String::from("master-1.x"));

        assert_eq!(service_config.volumes(), None);
    }

    #[test]
    fn should_parse_config_with_default_container_runtime() {
        let config_str = "";

        let config = from_str::<Config>(config_str).unwrap();

        let runtime = config.runtime_config();
        assert_eq!(runtime, Runtime::Docker);
    }
}
