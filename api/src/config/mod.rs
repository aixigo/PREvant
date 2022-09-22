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

pub use self::companion::DeploymentStrategy;
use self::companion::{Companion, CompanionType};
pub use self::container::ContainerConfig;
pub use self::runtime::Runtime;
use crate::models::ServiceConfig;
pub(self) use app_selector::AppSelector;
use clap::Parser;
use figment::providers::{Env, Format, Toml};
use figment::value::{Dict, Map, Tag, Value};
use figment::{Metadata, Profile};
pub(self) use secret::Secret;
use secstr::SecUtf8;
use std::collections::BTreeMap;
use std::convert::From;
use std::fmt::Display;
use std::io::Error as IOError;
use std::path::PathBuf;
use std::str::FromStr;
use toml::de::Error as TomlError;

mod app_selector;
mod companion;
mod container;
mod runtime;
mod secret;

#[derive(Default, Parser)]
#[clap(author, version, about, long_about = None)]
pub struct CliArgs {
    /// Sets a custom config file
    #[clap(short, long, value_parser, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Sets the container backend type, e.g. Docker or Kubernetes
    #[clap(short, long)]
    runtime_type: Option<RuntimeTypeCliFlag>,
}

enum RuntimeTypeCliFlag {
    Docker,
    Kubernetes,
}

impl FromStr for RuntimeTypeCliFlag {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Docker" => Ok(Self::Docker),
            "Kubernetes" => Ok(Self::Kubernetes),
            _ => Err("Unknown type"),
        }
    }
}

impl Display for RuntimeTypeCliFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            RuntimeTypeCliFlag::Docker => write!(f, "Docker"),
            RuntimeTypeCliFlag::Kubernetes => write!(f, "Kubernetes"),
        }
    }
}

impl figment::Provider for CliArgs {
    fn metadata(&self) -> Metadata {
        Metadata::named("cli arguments")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        let mut dict = Dict::new();

        if let Some(runtime_type) = &self.runtime_type {
            dict.insert(
                String::from("runtime"),
                figment::util::nest(
                    "type",
                    Value::String(Tag::Default, runtime_type.to_string()),
                ),
            );
        }

        let mut data = Map::new();
        data.insert(Profile::Default, dict);

        Ok(data)
    }
}

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
    #[serde(default)]
    runtime: Runtime,
    containers: Option<ContainerConfig>,
    jira: Option<JiraConfig>,
    companions: Option<BTreeMap<String, Companion>>,
    services: Option<BTreeMap<String, Service>>,
    hooks: Option<BTreeMap<String, PathBuf>>,
    #[serde(default)]
    registries: BTreeMap<String, Registry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct Registry {
    username: String,
    password: SecUtf8,
}

impl Config {
    pub fn from_figment(cli: &CliArgs) -> Result<Self, figment::Error> {
        figment::Figment::new()
            .merge(Toml::file(
                cli.config
                    .as_ref()
                    .unwrap_or(&PathBuf::from_str("config.toml").unwrap()),
            ))
            .merge(Env::prefixed("PREVANT_").split("_"))
            .merge(cli)
            .extract::<Config>()
    }

    pub fn runtime_config(&self) -> &Runtime {
        &self.runtime
    }

    pub fn container_config(&self) -> ContainerConfig {
        match &self.containers {
            Some(containers) => containers.clone(),
            None => ContainerConfig::default(),
        }
    }

    pub fn jira_config(&self) -> Option<JiraConfig> {
        self.jira.as_ref().cloned()
    }

    pub fn service_companion_configs(
        &self,
        app_name: &str,
    ) -> Vec<(ServiceConfig, DeploymentStrategy)> {
        self.companion_configs(app_name, |companion| {
            companion.companion_type() == &CompanionType::Service
        })
    }

    pub fn application_companion_configs(
        &self,
        app_name: &str,
    ) -> Vec<(ServiceConfig, DeploymentStrategy)> {
        self.companion_configs(app_name, |companion| {
            companion.companion_type() == &CompanionType::Application
        })
    }

    fn companion_configs<P>(
        &self,
        app_name: &str,
        predicate: P,
    ) -> Vec<(ServiceConfig, DeploymentStrategy)>
    where
        P: Fn(&Companion) -> bool,
    {
        match &self.companions {
            None => vec![],
            Some(companions_map) => companions_map
                .iter()
                .filter(|(_, companion)| companion.matches_app_name(app_name))
                .filter(|(_, companion)| predicate(*companion))
                .map(|(_, companion)| {
                    (
                        companion.clone().into(),
                        companion.deployment_strategy().clone(),
                    )
                })
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
        self.hooks.as_ref().and_then(|hooks| hooks.get(hook_name))
    }

    pub fn registry_credentials<'a, 'b: 'a>(
        &'b self,
        registry_host: &str,
    ) -> Option<(&'a str, &'a SecUtf8)> {
        self.registries
            .get(registry_host)
            .map(|registry| (registry.username.as_str(), &registry.password))
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

                service_config.add_file(path, sec);
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
    ( $config_str:expr ) => {{
        use figment::providers::Format;
        let provider = figment::providers::Toml::string($config_str);
        figment::Figment::from(provider)
            .extract::<crate::config::Config>()
            .unwrap()
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{service::ContainerType, Image};
    use std::str::FromStr;

    macro_rules! service_config {
        ( $name:expr ) => {{
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update($name);
            let img_hash = &format!("sha256:{:x}", hasher.finalize());

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
        companion_configs.iter().for_each(|(config, _)| {
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
        companion_configs.iter().for_each(|(config, _)| {
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
    fn should_return_service_companions_with_deployment_strategy() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'service'
            image = 'private.example.com/library/openid:latest'
            deploymentStrategy = 'redeploy-on-image-update'
            "#
        );

        let companion_configs = config.service_companion_configs("master");

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|(_, strategy)| {
            assert_eq!(strategy, &DeploymentStrategy::RedeployOnImageUpdate);
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
        companion_configs.iter().for_each(|(config, _)| {
            assert_eq!(config.files().unwrap().len(), 2);
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
        companion_configs.iter().for_each(|(config, _)| {
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
        companion_configs.iter().for_each(|(config, _)| {
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
            .files()
            .expect("File content is missing")
            .get(&PathBuf::from("/run/secrets/user"))
            .expect("No file for /run/secrets/user");
        assert_eq!(secret_file_content, &SecUtf8::from("Hello"));
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
            .files()
            .expect("File content is missing")
            .get(&PathBuf::from("/run/secrets/user"))
            .expect("No file for /run/secrets/user");
        assert_eq!(secret_file_content, &SecUtf8::from("Hello"));
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
            .files()
            .expect("File content is missing")
            .get(&PathBuf::from("/run/secrets/user"))
            .expect("No file for /run/secrets/user");
        assert_eq!(secret_file_content, &SecUtf8::from("Hello"));
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

        assert!(service_config.files().is_none());
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

        assert_eq!(service_config.files(), None);
    }

    #[test]
    fn should_parse_config_with_default_container_runtime() {
        let config = config_from_str!("");

        assert_eq!(config.runtime_config(), &Runtime::Docker);
    }

    #[test]
    fn should_convert_cli_to_config_via_figment() {
        let args = CliArgs::parse_from(["", "--runtime-type", "Kubernetes"]);

        let config = figment::Figment::new()
            .merge(args)
            .extract::<Config>()
            .unwrap();

        assert_eq!(
            config.runtime_config(),
            &Runtime::Kubernetes(Default::default())
        );
    }

    #[test]
    fn should_parse_registry_credentials() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "config.toml",
                r#"
                [registries.'docker.io']
                username = "user"
                password = "pass"
                "#,
            )?;

            let config = Config::from_figment(&Default::default())?;

            assert_eq!(
                config.registry_credentials("docker.io"),
                Some(("user", &SecUtf8::from_str("pass").unwrap()))
            );
            Ok(())
        })
    }

    #[test]
    fn should_return_application_companions_as_service_configs_with_volumes_as_files() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:11-alpine'
            env = [ 'KEY=VALUE' ]

            [companions.openid.files]
            '/tmp/test-1.json' = '{}'
            '/tmp/test-2.json' = '{}'
            "#
        );

        let companion_configs = config.application_companion_configs("master");

        assert_eq!(companion_configs.len(), 1);
        companion_configs.iter().for_each(|(config, _)| {
            assert_eq!(config.files().unwrap().len(), 2);
        });
    }
}
