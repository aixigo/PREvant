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
use crate::deployment::deployment_unit::{DeployableService, ServicePath};
use crate::models::AppName;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsValue, Source};
use std::path::Path;

use super::Hooks;

pub struct PersistenceHooks;

impl PersistenceHooks {
    pub async fn parse_and_run_hook(
        app_name: &AppName,
        services: &[DeployableService],
        hook_path: &Path,
    ) -> Result<Vec<Vec<ServicePath>>, AppsServiceError> {
        match Self::parse_hook(hook_path).await {
            Some(mut context) => {
                Self::register_configs_as_global_property(&mut context, services);
                context
                    .register_global_property(
                        "appName",
                        JsValue::String(app_name.to_string().into()),
                        Attribute::READONLY,
                    )
                    .expect("Property registration failed unexpectedly");

                let transformed_configs = context
                    .eval(Source::from_bytes(
                        "persistenceHook(appName, serviceConfigs)",
                    ))
                    .unwrap();

                let transformed_configs = transformed_configs.to_json(&mut context).unwrap();
                Self::parse_service_config(transformed_configs)
            }
            None => Ok(Hooks::default_persistence_structure(services)),
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

        if let Err(err) = context.eval(Source::from_bytes(&hook_content)) {
            error!(
                "Cannot populate hook {:?} to Javascript context: {:?}",
                hook_path, err
            );
            return None;
        }

        if dbg!(context.interner().get("persistenceHook")).is_some() {
            Some(context)
        } else {
            None
        }
    }

    fn register_configs_as_global_property(context: &mut Context, services: &[DeployableService]) {
        let js_configs = services
            .iter()
            .flat_map(|service| match service.declared_volumes().is_empty() {
                true => vec![JsPersistenceConfig::new(service.service_name(), None)],
                false => service
                    .declared_volumes()
                    .iter()
                    .map(|volume| JsPersistenceConfig::new(service.service_name(), Some(volume)))
                    .collect::<Vec<_>>(),
            })
            .collect::<Vec<_>>();
        let js_configs = serde_json::to_value(js_configs).expect("Should be serializable");
        let js_configs =
            JsValue::from_json(&js_configs, context).expect("Unable to read JSON value");

        context
            .register_global_property("serviceConfigs", js_configs, Attribute::READONLY)
            .expect("Property registration failed unexpectedly");
    }

    fn parse_service_config(
        transformed_configs: serde_json::value::Value,
    ) -> Result<Vec<Vec<ServicePath>>, AppsServiceError> {
        serde_json::from_value::<Vec<Vec<ServicePath>>>(transformed_configs).map_err(|err| {
            error!("Cannot parse result of persistence hook: {}", err);
            AppsServiceError::InvalidPersistenceHook
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsPersistenceConfig {
    name: String,
    #[serde(default)]
    path: Option<String>,
}

impl JsPersistenceConfig {
    fn new(name: &str, path: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            path: path.map(|s| s.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::*;
    use crate::config::Config;
    use crate::deployment::deployment_unit::DeploymentStrategy;
    use crate::deployment::hooks::Hooks;
    use crate::infrastructure::TraefikIngressRoute;
    use std::io::Write;
    use std::str::FromStr;
    use std::vec;
    use tempfile::NamedTempFile;

    fn config_with_persistence_hook(script: &str) -> (NamedTempFile, Config) {
        let mut hook_file = NamedTempFile::new().unwrap();

        hook_file.write_all(script.as_bytes()).unwrap();

        let config = crate::config_from_str!(&format!(
            r#"
            [hooks]
            persistence = {:?}
            "#,
            hook_file.path()
        ));

        (hook_file, config)
    }

    #[tokio::test]
    async fn apply_persistence_hook_to_share_volumes() -> Result<(), AppsError> {
        let script = r#"
        function persistenceHook(appName, configs) {
            let sharedGroups = [
                [
                    { serviceName: "service-a", path: "/var/lib/data" },
                    { serviceName: "service-b", path: "/var/lib/data" }
                ],
                [
                    { serviceName: "service-a", path: "/var/lib/cache" },
                    { serviceName: "service-c", path: "/var/lib/cache" }
                ]
            ];
            let result = sharedGroups.map(() => []);
            configs.forEach(config => {
                let matched = false;
                sharedGroups.forEach((group, groupIndex) => {
                    if (group.some(pair => pair.serviceName === config.name && pair.path === config.path)) {
                        result[groupIndex].push({ serviceName: config.name, path: config.path });
                        matched = true;
                    }
                });
            });
            return result.filter(group => group.length > 0);
        }        
        "#;

        let (_temp_js_file, config) = config_with_persistence_hook(script);

        let persistence_hooks = Hooks::new(&config);
        let services = vec![
            crate::sc!("service-a"),
            crate::sc!("service-b"),
            crate::sc!("service-c"),
        ];
        let deployable_services = services
            .into_iter()
            .map(|service| {
                DeployableService::new(
                    service,
                    DeploymentStrategy::RedeployAlways,
                    TraefikIngressRoute::empty(),
                    vec![
                        String::from("/var/lib/data"),
                        String::from("/var/lib/cache"),
                    ],
                )
            })
            .collect::<Vec<_>>();

        let result = persistence_hooks
            .apply_persistence_hooks(&AppName::from_str("master").unwrap(), &deployable_services)
            .await?;

        assert_eq!(
            result,
            vec![
                vec![
                    ServicePath::new(String::from("service-a"), String::from("/var/lib/data")),
                    ServicePath::new(String::from("service-b"), String::from("/var/lib/data"))
                ],
                vec![
                    ServicePath::new(String::from("service-a"), String::from("/var/lib/cache")),
                    ServicePath::new(String::from("service-c"), String::from("/var/lib/cache"))
                ]
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn apply_persistence_hook_to_add_missing_volumes() -> Result<(), AppsError> {
        let script = r#"
        function persistenceHook( appName, configs ) {
            let result = [];
            configs.forEach(config => {
                [{serviceName: "service-a", path: "/var/lib/data"}].forEach(volume => {
                    if (volume.serviceName === config.name) {
                        result.push([{ serviceName: volume.serviceName, path: volume.path }]);
                    }
                });
            })

            return result;
        }
        "#;

        let (_temp_js_file, config) = config_with_persistence_hook(script);

        let persistence_hooks = Hooks::new(&config);
        let deployable_services = vec![DeployableService::new(
            crate::sc!("service-a"),
            DeploymentStrategy::RedeployNever,
            TraefikIngressRoute::empty(),
            Vec::new(),
        )];

        let result = persistence_hooks
            .apply_persistence_hooks(&AppName::from_str("master").unwrap(), &deployable_services)
            .await?;

        assert_eq!(
            result,
            vec![vec![ServicePath::new(
                String::from("service-a"),
                String::from("/var/lib/data")
            )]]
        );

        Ok(())
    }

    #[tokio::test]
    async fn fail_with_persistence_hook_returning_invalid_object() -> Result<(), AppsError> {
        let script = r#"
        function persistenceHook( appName, configs ) {
            return 'unexpected return value';
        }        
        "#;

        let (_temp_js_file, config) = config_with_persistence_hook(script);

        let persistence_hooks = Hooks::new(&config);
        let deployable_services = DeployableService::new(
            crate::sc!("service-a"),
            DeploymentStrategy::RedeployAlways,
            TraefikIngressRoute::empty(),
            vec![
                String::from("/var/lib/data"),
                String::from("/var/lib/cache"),
            ],
        );

        let mut persistence_hook_error = String::new();
        match persistence_hooks
            .apply_persistence_hooks(
                &AppName::from_str("master").unwrap(),
                &vec![deployable_services],
            )
            .await
        {
            Ok(result) => Some(result),
            Err(err) => {
                persistence_hook_error = err.to_string();
                None
            }
        };

        assert_eq!(
            persistence_hook_error,
            String::from("Invalid persistence hook.")
        );

        Ok(())
    }

    #[tokio::test]
    async fn apply_persistence_hook_to_delete_declared_volumes() -> Result<(), AppsError> {
        let script = r#"
        function persistenceHook(appName, configs) {
            let result = [];
            for (const config of configs) {
                for (const volume of [{ serviceName: "service-a", path: "/var/lib/data" }]) {
                    if (volume.serviceName === config.name && volume.path === config.path) {
                        break;
                    } else {
                        result.push([{ serviceName: config.name, path: config.path }]);
                    }
                }
            }
            return result;
        }
             
        "#;

        let (_temp_js_file, config) = config_with_persistence_hook(script);

        let persistence_hooks = Hooks::new(&config);
        let services = vec![crate::sc!("service-a"), crate::sc!("service-b")];

        let deployable_services = services
            .into_iter()
            .map(|service| {
                DeployableService::new(
                    service,
                    DeploymentStrategy::RedeployAlways,
                    TraefikIngressRoute::empty(),
                    vec![
                        String::from("/var/lib/data"),
                        String::from("/var/lib/cache"),
                    ],
                )
            })
            .collect::<Vec<_>>();

        let result = persistence_hooks
            .apply_persistence_hooks(&AppName::from_str("master").unwrap(), &deployable_services)
            .await?;

        assert_eq!(
            result,
            vec![
                vec![ServicePath::new(
                    "service-a".to_string(),
                    "/var/lib/cache".to_string()
                )],
                vec![ServicePath::new(
                    "service-b".to_string(),
                    "/var/lib/data".to_string()
                )],
                vec![ServicePath::new(
                    "service-b".to_string(),
                    "/var/lib/cache".to_string()
                )]
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn default_persistence_structure_in_case_of_missing_persistence_hook(
    ) -> Result<(), AppsError> {
        let script = r#"
        function deploymentHook(appName, configs) {
            return configs;
        }   
        "#;

        let (_temp_js_file, config) = config_with_persistence_hook(script);

        let persistence_hooks = Hooks::new(&config);
        let services = vec![crate::sc!("service-a"), crate::sc!("service-b")];

        let deployable_services = services
            .into_iter()
            .map(|service| {
                DeployableService::new(
                    service,
                    DeploymentStrategy::RedeployAlways,
                    TraefikIngressRoute::empty(),
                    vec![
                        String::from("/var/lib/data"),
                        String::from("/var/lib/cache"),
                    ],
                )
            })
            .collect::<Vec<_>>();

        let result = persistence_hooks
            .apply_persistence_hooks(&AppName::from_str("master").unwrap(), &deployable_services)
            .await?;

        assert_eq!(
            result,
            vec![
                vec![ServicePath::new(
                    "service-a".to_string(),
                    "/var/lib/data".to_string()
                )],
                vec![ServicePath::new(
                    "service-a".to_string(),
                    "/var/lib/cache".to_string()
                )],
                vec![ServicePath::new(
                    "service-b".to_string(),
                    "/var/lib/data".to_string()
                )],
                vec![ServicePath::new(
                    "service-b".to_string(),
                    "/var/lib/cache".to_string()
                )]
            ]
        );

        Ok(())
    }
}
