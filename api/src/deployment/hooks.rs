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
use crate::apps::AppsServiceError;
use crate::config::Config;
use crate::models::{AppName, ContainerType, Environment, EnvironmentVariable, Image};
use boa_engine::property::Attribute;
use boa_engine::syntax::ast::node::Node;
use boa_engine::{Context, JsValue};
use secstr::SecUtf8;
use std::collections::BTreeMap;
use std::iter::IntoIterator;
use std::path::{Path, PathBuf};

use super::deployment_unit::DeployableService;

pub struct Hooks<'a> {
    hook_config: &'a Config,
}

impl<'a> Hooks<'a> {
    pub fn new(hook_config: &'a Config) -> Self {
        Hooks { hook_config }
    }

    pub async fn apply_deployment_hook(
        &self,
        app_name: &AppName,
        services: Vec<DeployableService>,
    ) -> Result<Vec<DeployableService>, AppsServiceError> {
        match self.hook_config.hook("deployment") {
            None => Ok(services),
            Some(hook_path) => self.parse_and_run_hook(app_name, services, hook_path).await,
        }
    }

    async fn parse_and_run_hook(
        &self,
        app_name: &AppName,
        services: Vec<DeployableService>,
        hook_path: &Path,
    ) -> Result<Vec<DeployableService>, AppsServiceError> {
        match Self::parse_hook(hook_path).await {
            Some(mut context) => {
                Self::register_configs_as_global_property(&mut context, &services);
                context.register_global_property(
                    "appName",
                    JsValue::String(app_name.to_string().into()),
                    Attribute::READONLY,
                );

                let transformed_configs = context
                    .eval("deploymentHook(appName, serviceConfigs)")
                    .unwrap();

                let transformed_configs = transformed_configs.to_json(&mut context).unwrap();

                Self::parse_service_config(services, transformed_configs)
            }
            None => Ok(services),
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
        services: &[DeployableService],
    ) {
        let js_configs = services
            .iter()
            .map(JsServiceConfig::from)
            .collect::<Vec<_>>();

        let js_configs = serde_json::to_value(js_configs).expect("Should be serializable");
        let js_configs =
            JsValue::from_json(&js_configs, &mut context).expect("Unable to read JSON value");

        context.register_global_property("serviceConfigs", js_configs, Attribute::READONLY);
    }

    fn parse_service_config<Iter>(
        services: Iter,
        transformed_configs: serde_json::value::Value,
    ) -> Result<Vec<DeployableService>, AppsServiceError>
    where
        Iter: IntoIterator<Item = DeployableService>,
    {
        let mut transformed_configs =
            serde_json::from_value::<Vec<JsServiceConfig>>(transformed_configs).map_err(|err| {
                error!("Cannot parse result of deployment hook: {}", err);
                AppsServiceError::InvalidDeploymentHook
            })?;

        Ok(services
            .into_iter()
            .filter_map(move |service| {
                let index = transformed_configs.iter().position(|transformed_config| {
                    &transformed_config.name == service.service_name()
                        && &transformed_config.r#type == service.container_type()
                        && &transformed_config.image == service.image()
                })?;

                let transformed_config = transformed_configs.swap_remove(index);

                Some(transformed_config.apply_to(service))
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
    files: BTreeMap<PathBuf, SecUtf8>,
    r#type: ContainerType,
}

impl JsServiceConfig {
    fn apply_to(mut self, mut service: DeployableService) -> DeployableService {
        service.set_files(Some(self.files));

        let env = match service.env().cloned() {
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

        service.set_env(env);

        service
    }
}

impl From<&DeployableService> for JsServiceConfig {
    fn from(config: &DeployableService) -> Self {
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
            files: config.files().cloned().unwrap_or_default(),
            r#type: config.container_type().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::*;
    use crate::deployment::deployment_unit::DeploymentUnitBuilder;
    use std::io::Write;
    use std::str::FromStr;
    use std::vec;
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

        let app_name = AppName::from_str("master").unwrap();
        let service_configs = vec![crate::sc!("service-a")];

        let unit = DeploymentUnitBuilder::init(app_name, service_configs)
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .resolve_image_manifest(&config)
            .await?
            .apply_templating()?
            .apply_hooks(&config)
            .await?
            .build();

        let deployed_files = unit
            .services()
            .into_iter()
            .map(|service| service.files().cloned())
            .flatten()
            .flatten()
            .map(|(path, content)| (path.to_str().unwrap().to_string(), content.clone()))
            .collect::<Vec<(String, SecUtf8)>>();

        assert_eq!(
            deployed_files,
            vec![(
                String::from("/etc/some-config.txt"),
                SecUtf8::from("service-a0")
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

        let app_name = AppName::from_str("master").unwrap();

        let mut service_config = crate::sc!("service-a");
        let mut files = BTreeMap::new();
        files.insert(
            PathBuf::from("/etc/some-config.txt"),
            SecUtf8::from("value"),
        );
        service_config.set_files(Some(files));

        let unit = DeploymentUnitBuilder::init(app_name, vec![service_config])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .resolve_image_manifest(&config)
            .await?
            .apply_templating()?
            .apply_hooks(&config)
            .await?
            .build();

        let deployed_files = unit
            .services()
            .into_iter()
            .map(|service| service.files().cloned())
            .flatten()
            .flatten()
            .map(|(path, content)| (path.to_str().unwrap().to_string(), content.clone()))
            .collect::<Vec<(String, SecUtf8)>>();

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
        let app_name = AppName::from_str("master").unwrap();
        let service_config = crate::sc!("service-a");

        let unit = DeploymentUnitBuilder::init(app_name, vec![service_config])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .resolve_image_manifest(&config)
            .await?
            .apply_templating()?
            .apply_hooks(&config)
            .await?
            .build();

        let deployed_variables = unit
            .services()
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
        let app_name = AppName::from_str("master").unwrap();

        let mut service_config = crate::sc!("service-a");
        service_config.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("VARIABLE_X"),
            SecUtf8::from("Hello"),
        )])));

        let unit = DeploymentUnitBuilder::init(app_name, vec![service_config])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .resolve_image_manifest(&config)
            .await?
            .apply_templating()?
            .apply_hooks(&config)
            .await?
            .build();

        let deployed_variables = unit
            .services()
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

        let app_name = AppName::from_str("master").unwrap();
        let mut service_config = crate::sc!("service-a");
        service_config.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("VARIABLE_X"),
            SecUtf8::from("Hello"),
        )])));

        let unit = DeploymentUnitBuilder::init(app_name, vec![service_config])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .resolve_image_manifest(&config)
            .await?
            .apply_templating()?
            .apply_hooks(&config)
            .await?
            .build();

        let deployed_variables = unit
            .services()
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
        let service_config = crate::sc!("service-a");
        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let app_name = AppName::from_str("master").unwrap();

        let unit = DeploymentUnitBuilder::init(app_name, vec![service_config])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .resolve_image_manifest(&config)
            .await?
            .apply_templating()?
            .apply_hooks(&config)
            .await?
            .build();

        let deployed_services = unit
            .services()
            .into_iter()
            .map(|services| services.service_name().clone())
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

        let service_config = crate::sc!("service-a");
        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let app_name = AppName::from_str("master").unwrap();

        let unit = DeploymentUnitBuilder::init(app_name, vec![service_config])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .resolve_image_manifest(&config)
            .await?
            .apply_templating()?
            .apply_hooks(&config)
            .await?
            .build();

        let deployed_services = unit.services();

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
        let mut deployed_services_error = String::new();
        let service_config = crate::sc!("service-a");
        let (_temp_js_file, config) = config_with_deployment_hook(script);
        let app_name = AppName::from_str("master").unwrap();
        match DeploymentUnitBuilder::init(app_name, vec![service_config])
            .extend_with_config(&config)
            .extend_with_templating_only_service_configs(Vec::new())
            .resolve_image_manifest(&config)
            .await?
            .apply_templating()?
            .apply_hooks(&config)
            .await
        {
            Ok(no_error) => Some(no_error),
            Err(err) => {
                deployed_services_error = err.to_string();
                None
            }
        };

        assert_eq!(
            deployed_services_error,
            String::from("Invalid deployment hook.")
        );

        Ok(())
    }
}
