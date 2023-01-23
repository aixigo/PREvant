/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2021 aixigo AG
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
use crate::apps::{Apps, AppsServiceError};
use crate::infrastructure::DeploymentStrategy;
use crate::models::{AppName, ContainerType, Environment, EnvironmentVariable, Image};

use boa_engine::property::Attribute;
use boa_engine::syntax::ast::node::Node;
use boa_engine::{Context, JsValue};
use secstr::SecUtf8;
use std::collections::BTreeMap;
use std::iter::IntoIterator;
use std::path::{Path, PathBuf};

impl Apps {
    pub(super) async fn apply_deployment_hook(
        &self,
        app_name: &AppName,
        configs: Vec<DeploymentStrategy>,
    ) -> Result<Vec<DeploymentStrategy>, AppsServiceError> {
        match self.config.hook("deployment") {
            None => Ok(configs),
            Some(hook_path) => self.parse_and_run_hook(app_name, configs, hook_path).await,
        }
    }

    async fn parse_and_run_hook(
        &self,
        app_name: &AppName,
        configs: Vec<DeploymentStrategy>,
        hook_path: &Path,
    ) -> Result<Vec<DeploymentStrategy>, AppsServiceError> {
        match Self::parse_hook(hook_path).await {
            Some(mut context) => {
                Self::register_configs_as_global_property(&mut context, &configs);
                context.register_global_property(
                    "appName",
                    JsValue::String(app_name.to_string().into()),
                    Attribute::READONLY,
                );

                let transformed_configs = context
                    .eval("deploymentHook(appName, serviceConfigs)")
                    .unwrap();

                let transformed_configs = transformed_configs.to_json(&mut context).unwrap();

                Self::parse_service_config(configs, transformed_configs)
            }
            None => Ok(configs),
        }
    }

    async fn parse_hook(hook_path: &Path) -> Option<Context> {
        let hook_content = match tokio::fs::read_to_string(hook_path).await {
            Ok(hook_content) => hook_content,
            Err(err) => {
                error!("Cannot read hook file {:?}: {}", hook_path, err);
                return None;
            }
        };

        let mut context = Context::default();
        let statements = match context.parse(&hook_content) {
            Ok(statements) => statements,
            Err(err) => {
                error!("Cannot parse hook file {:?}: {}", hook_path, err);
                return None;
            }
        };

        if let Err(err) = context.eval(&hook_content) {
            error!(
                "Cannot populate hook {:?} to Javascript context: {:?}",
                hook_path, err
            );
            return None;
        }

        statements
            .items()
            .iter()
            .find_map(|node| match node {
                Node::FunctionDecl(decl)
                    if context.interner().resolve(decl.name()) == Some("deploymentHook") =>
                {
                    Some(decl)
                }
                _ => None,
            })
            .map(|_| context)
    }

    fn register_configs_as_global_property(
        mut context: &mut Context,
        configs: &[DeploymentStrategy],
    ) {
        let js_configs = configs
            .iter()
            .map(JsServiceConfig::from)
            .collect::<Vec<_>>();

        let js_configs = serde_json::to_value(js_configs).expect("Should be serializable");
        let js_configs =
            JsValue::from_json(&js_configs, &mut context).expect("Unable to read JSON value");

        context.register_global_property("serviceConfigs", js_configs, Attribute::READONLY);
    }

    fn parse_service_config<Iter>(
        configs: Iter,
        transformed_configs: serde_json::value::Value,
    ) -> Result<Vec<DeploymentStrategy>, AppsServiceError>
    where
        Iter: IntoIterator<Item = DeploymentStrategy>,
    {
        let mut transformed_configs =
            serde_json::from_value::<Vec<JsServiceConfig>>(transformed_configs).map_err(|err| {
                error!("Cannot parse result of deployment hook: {}", err);
                AppsServiceError::InvalidDeploymentHook
            })?;

        Ok(configs
            .into_iter()
            .filter_map(move |config| {
                let index = transformed_configs.iter().position(|transformed_config| {
                    &transformed_config.name == config.service_name()
                        && &transformed_config.r#type == config.container_type()
                        && &transformed_config.image == config.image()
                })?;

                let transformed_config = transformed_configs.swap_remove(index);

                Some(transformed_config.apply_to(config))
            })
            .collect::<Vec<_>>())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsServiceConfig {
    name: String,
    image: Image,
    #[serde(default)]
    env: BTreeMap<String, SecUtf8>,
    #[serde(default)]
    files: BTreeMap<PathBuf, String>,
    r#type: ContainerType,
}

impl JsServiceConfig {
    fn apply_to(mut self, mut config: DeploymentStrategy) -> DeploymentStrategy {
        config.set_volumes(Some(self.files));

        let env = match config.env().cloned() {
            Some(env) => {
                let mut variables = Vec::new();

                for ev in env.into_iter() {
                    if let Some(value) = self.env.remove(ev.key()) {
                        variables.push(ev.with_value(value));
                    }
                }
                variables.extend(
                    self.env
                        .into_iter()
                        .map(|(key, value)| EnvironmentVariable::new(key, value)),
                );

                Some(Environment::new(variables))
            }
            None if !self.env.is_empty() => {
                let variables = self
                    .env
                    .into_iter()
                    .map(|(key, value)| EnvironmentVariable::new(key, value))
                    .collect();
                Some(Environment::new(variables))
            }
            env => env,
        };

        config.set_env(env);

        config
    }
}

impl From<&DeploymentStrategy> for JsServiceConfig {
    fn from(config: &DeploymentStrategy) -> Self {
        Self {
            name: config.service_name().clone(),
            image: config.image().clone(),
            env: config
                .env()
                .map(|env| {
                    env.iter()
                        .map(|v| (v.key().clone(), v.value().clone()))
                        .collect()
                })
                .unwrap_or_default(),
            files: config.volumes().cloned().unwrap_or_default(),
            r#type: config.container_type().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::*;
    use crate::config::Config;
    use crate::infrastructure::Dummy;
    use std::collections::BTreeMap;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn config_with_deployment_hook(script: &str) -> (NamedTempFile, Config) {
        let mut hook_file = NamedTempFile::new().unwrap();

        hook_file.write_all(script.as_bytes()).unwrap();

        let config = crate::config_from_str!(&format!(
            r#"
            [hooks]
            deployment = {:?}
            "#,
            hook_file.path()
        ));

        (hook_file, config)
    }

    #[tokio::test]
    async fn apply_deployment_hook() -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            if( appName === 'master' ) {
                return configs.filter( service => service.name !== 'service-b' );
            }
            return configs;
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &[crate::sc!("service-a"), crate::sc!("service-b")],
        )
        .await?;

        let deployed_services = apps
            .get_apps()
            .await?
            .into_iter()
            .map(|(_, services)| services.into_iter().map(|s| s.service_name().clone()))
            .flatten()
            .collect::<Vec<_>>();

        assert_eq!(deployed_services, vec![String::from("service-a")]);

        Ok(())
    }

    #[tokio::test]
    async fn apply_deployment_hook_with_file_modification() -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            return configs.map((config, index) => {
                config.files['/etc/some-config.txt'] = config.name + index;
                return config;
            });
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &[crate::sc!("service-a")],
        )
        .await?;

        let services = apps
            .infrastructure
            .get_configs_of_app("master")
            .await
            .unwrap();
        let deployed_files = services
            .into_iter()
            .map(|service| service.volumes().cloned())
            .flatten()
            .flatten()
            .map(|(path, content)| (path.to_str().unwrap().to_string(), content))
            .collect::<Vec<(String, String)>>();

        assert_eq!(
            deployed_files,
            vec![(
                String::from("/etc/some-config.txt"),
                String::from("service-a0")
            )]
        );

        Ok(())
    }

    #[tokio::test]
    async fn apply_deployment_hook_with_file_removal() -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            return configs.map((config, index) => {
                delete config.files['/etc/some-config.txt'];
                return config;
            });
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let mut service_config = crate::sc!("service-a");
        let mut volumes = BTreeMap::new();
        volumes.insert(PathBuf::from("/etc/some-config.txt"), String::from("value"));
        service_config.set_volumes(Some(volumes));

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &[service_config],
        )
        .await?;

        let services = apps
            .infrastructure
            .get_configs_of_app("master")
            .await
            .unwrap();
        let deployed_files = services
            .into_iter()
            .map(|service| service.volumes().cloned())
            .flatten()
            .flatten()
            .map(|(path, content)| (path.to_str().unwrap().to_string(), content))
            .collect::<Vec<(String, String)>>();

        assert_eq!(deployed_files, vec![]);

        Ok(())
    }

    #[tokio::test]
    async fn apply_deployment_hook_add_env() -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            return configs.map((config, index) => {
                config.env['VARIABLE_X'] = config.name + index;
                return config;
            });
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &[crate::sc!("service-a")],
        )
        .await?;

        let services = apps
            .infrastructure
            .get_configs_of_app("master")
            .await
            .unwrap();
        let deployed_variables = services
            .into_iter()
            .map(|service| service.env().cloned())
            .flatten()
            .map(|env| env.into_iter())
            .flatten()
            .map(|env| (env.key().clone(), env.value().unsecure().to_string()))
            .collect::<Vec<(String, String)>>();

        assert_eq!(
            deployed_variables,
            vec![(String::from("VARIABLE_X"), String::from("service-a0"))]
        );

        Ok(())
    }

    #[tokio::test]
    async fn apply_deployment_hook_with_env_modification() -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            return configs.map((config, index) => {
                config.env['VARIABLE_Y'] = config.name + index;
                return config;
            });
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let mut service_config = crate::sc!("service-a");
        service_config.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("VARIABLE_X"),
            SecUtf8::from("Hello"),
        )])));

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &[service_config],
        )
        .await?;

        let services = apps
            .infrastructure
            .get_configs_of_app("master")
            .await
            .unwrap();
        let deployed_variables = services
            .into_iter()
            .map(|service| service.env().cloned())
            .flatten()
            .map(|env| env.into_iter())
            .flatten()
            .map(|env| (env.key().clone(), env.value().unsecure().to_string()))
            .collect::<Vec<(String, String)>>();

        assert_eq!(
            deployed_variables,
            vec![
                (String::from("VARIABLE_X"), String::from("Hello")),
                (String::from("VARIABLE_Y"), String::from("service-a0"))
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn apply_deployment_hook_with_env_removal() -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            return configs.map((config, index) => {
                delete config.env['VARIABLE_X'];
                return config;
            });
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let mut service_config = crate::sc!("service-a");
        service_config.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("VARIABLE_X"),
            SecUtf8::from("Hello"),
        )])));

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &[service_config],
        )
        .await?;

        let services = apps
            .infrastructure
            .get_configs_of_app("master")
            .await
            .unwrap();
        let deployed_variables = services
            .into_iter()
            .map(|service| service.env().cloned())
            .flatten()
            .map(|env| env.into_iter())
            .flatten()
            .map(|env| (env.key().clone(), env.value().unsecure().to_string()))
            .collect::<Vec<(String, String)>>();

        assert_eq!(deployed_variables, vec![]);

        Ok(())
    }

    #[tokio::test]
    async fn apply_deployment_hook_without_adding_additional_services() -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            configs.push({
                name: 'hello',
                image: 'hello-world',
                type: 'instance'
            });
            return configs;
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &[crate::sc!("service-a")],
        )
        .await?;

        let deployed_services = apps
            .get_apps()
            .await?
            .into_iter()
            .map(|(_, services)| services.into_iter().map(|s| s.service_name().clone()))
            .flatten()
            .collect::<Vec<_>>();

        assert_eq!(deployed_services, vec![String::from("service-a")]);

        Ok(())
    }

    #[tokio::test]
    async fn do_not_apply_deployment_hook_when_modifying_immutable_values() -> Result<(), AppsError>
    {
        let script = r#"
        function deploymentHook( appName, configs ) {
            return configs.map((config, index) => {
                config.name = config.name + index;
                return config;
            });
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        apps.create_or_update(
            &AppName::from_str("master").unwrap(),
            &AppStatusChangeId::new(),
            None,
            &[crate::sc!("service-a")],
        )
        .await?;

        let deployed_services = apps.get_apps().await?;

        assert!(deployed_services.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn fail_with_hook_returning_invalid_object() -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook( appName, configs ) {
            return 'unexpected return value';
        }
        "#;

        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let infrastructure = Box::new(Dummy::new());
        let apps = AppsService::new(config, infrastructure)?;

        let deployed_services_error = apps
            .create_or_update(
                &AppName::from_str("master").unwrap(),
                &AppStatusChangeId::new(),
                None,
                &[crate::sc!("service-a"), crate::sc!("service-b")],
            )
            .await
            .unwrap_err()
            .to_string();

        assert_eq!(
            deployed_services_error,
            String::from("Invalid deployment hook.")
        );

        Ok(())
    }
}
