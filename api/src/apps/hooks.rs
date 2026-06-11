use crate::auth::User;
use crate::config::Config;
use boa_engine::{Context, JsValue, Source, property::Attribute};
use domain::{
    AppName, Image, Owner,
    app_blueprints::{Environment, EnvironmentVariable},
    app_deployment::DeployableService,
    app_instance::ContainerType,
};
use log::error;
use openidconnect::IdTokenClaims;
use openidconnect::core::CoreGenderClaim;
use secstr::SecUtf8;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::iter::IntoIterator;
use std::path::PathBuf;

pub struct Hooks<'a> {
    config: &'a Config,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq, Serialize, Deserialize)]
pub enum HooksError {
    #[error("Invalid deployment hook, {err}")]
    InvalidDeploymentHook { err: String },
    #[error("Invalid deployment hook: the hook script did not return anything.")]
    DeploymentHookReturnedNothing,
    #[error("Invalid user-to-owner hook execution: {err}")]
    UserToOwner { err: String },
    #[error("Invalid user-to-owner hook execution: the hook script did not return anything.")]
    UserToOwnerReturnedNothing,
    #[error("Unexpected err during hook execution: {err}")]
    Unexpected { err: String },
    #[error("Expected function {expected} is not present in hook file {file}")]
    ExpectedFunctionNotPresent { expected: String, file: PathBuf },
}

impl<'a> Hooks<'a> {
    pub fn new(config: &'a Config) -> Self {
        Hooks { config }
    }

    pub async fn apply_id_token_claims_to_owner_hook(
        &self,
        user: User,
    ) -> Result<Option<Owner>, HooksError> {
        let User::Oidc { id_token_claims } = user else {
            return Ok(None);
        };

        let Some(id_token_claims_to_owner_hook_path) =
            self.config.hook("idTokenClaimsToOwner").cloned()
        else {
            return Ok(Some(Owner {
                sub: id_token_claims.subject().clone(),
                iss: id_token_claims.issuer().clone(),
                name: id_token_claims
                    .name()
                    .and_then(|name| name.get(None))
                    .map(|name| name.to_string()),
            }));
        };

        let hook_content = match tokio::fs::read_to_string(&id_token_claims_to_owner_hook_path)
            .await
        {
            Ok(hook_content) => hook_content,
            Err(err) => {
                let err =
                    format!("Cannot read hook file {id_token_claims_to_owner_hook_path:?}: {err}");
                log::error!("{err}");
                return Err(HooksError::UserToOwner { err });
            }
        };

        let join = tokio::task::spawn_blocking(move || {
            let context = Self::parse_hook(
                id_token_claims_to_owner_hook_path.to_path_buf(),
                &hook_content,
                "idTokenClaimsToOwnerHook",
            )?;
            Self::run_id_token_claims_to_owner_hook(id_token_claims, context)
        });

        join.await
            .map_err(|err| HooksError::Unexpected {
                err: err.to_string(),
            })?
            .map(Some)
    }

    fn run_id_token_claims_to_owner_hook(
        id_token_claims: IdTokenClaims<crate::auth::AdditionalClaims, CoreGenderClaim>,
        mut context: Context,
    ) -> Result<Owner, HooksError> {
        let id_token_claims = JsValue::from_json(
            &serde_json::to_value(&id_token_claims).expect("Unable to serialize token claims"),
            &mut context,
        )
        .expect("Unable to read JSON value");

        context
            .register_global_property(
                boa_engine::js_string!("idTokenClaims"),
                id_token_claims,
                Attribute::READONLY,
            )
            .expect("Property registration failed unexpectedly");

        let owner = context
            .eval(Source::from_bytes(
                "idTokenClaimsToOwnerHook(idTokenClaims)",
            ))
            .map_err(|err| HooksError::UserToOwner {
                err: format!("Cannot execute hook: {err}"),
            })?;

        serde_json::from_value::<Owner>(
            owner
                .to_json(&mut context)
                .map_err(|e| HooksError::UserToOwner { err: e.to_string() })?
                .ok_or(HooksError::UserToOwnerReturnedNothing)?,
        )
        .map_err(|err| HooksError::UserToOwner {
            err: err.to_string(),
        })
    }

    pub async fn apply_deployment_hook(
        &self,
        app_name: &AppName,
        services: Vec<DeployableService>,
    ) -> Result<Vec<DeployableService>, HooksError> {
        let Some(deployment_hook_path) = self.config.hook("deployment").cloned() else {
            return Ok(services);
        };

        let hook_content = match tokio::fs::read_to_string(&deployment_hook_path).await {
            Ok(hook_content) => hook_content,
            Err(err) => {
                let err = format!("Cannot read hook file {deployment_hook_path:?}: {err}");
                error!("{err}");
                return Err(HooksError::InvalidDeploymentHook { err });
            }
        };

        let app_name = app_name.clone();
        let join = tokio::task::spawn_blocking(move || {
            let context = Self::parse_hook(
                deployment_hook_path.to_path_buf(),
                &hook_content,
                "deploymentHook",
            )?;
            Self::run_deployment_hook(app_name, services, context)
        });

        join.await.map_err(|err| HooksError::Unexpected {
            err: err.to_string(),
        })?
    }

    fn run_deployment_hook(
        app_name: AppName,
        services: Vec<DeployableService>,
        mut context: Context,
    ) -> Result<Vec<DeployableService>, HooksError> {
        Self::register_configs_as_global_property(&mut context, &services);
        context
            .register_global_property(
                boa_engine::js_string!("appName"),
                JsValue::new(boa_engine::js_string!(app_name.as_str())),
                Attribute::READONLY,
            )
            .expect("Property registration failed unexpectedly");

        let transformed_configs = context
            .eval(Source::from_bytes(
                "deploymentHook(appName, serviceConfigs)",
            ))
            .map_err(|err| HooksError::InvalidDeploymentHook {
                err: format!("Cannot execute hook: {err}"),
            })?;

        let transformed_configs = transformed_configs
            .to_json(&mut context)
            .map_err(|e| HooksError::InvalidDeploymentHook { err: e.to_string() })?
            .ok_or(HooksError::DeploymentHookReturnedNothing)?;

        Self::parse_service_config(services, transformed_configs)
    }

    fn parse_hook(
        hook_path: PathBuf,
        hook_content: &str,
        function_name: &str,
    ) -> Result<Context, HooksError> {
        let mut context = Context::default();

        if let Err(err) = context.eval(Source::from_bytes(&hook_content)) {
            error!(
                "Cannot populate hook {:?} to Javascript context: {:?}",
                hook_path, err
            );
            return Err(HooksError::Unexpected {
                err: err.to_string(),
            });
        }

        if context.interner().get(function_name).is_some() {
            Ok(context)
        } else {
            Err(HooksError::ExpectedFunctionNotPresent {
                expected: function_name.to_string(),
                file: hook_path,
            })
        }
    }

    fn register_configs_as_global_property(context: &mut Context, services: &[DeployableService]) {
        let js_configs = services
            .iter()
            .map(JsServiceConfig::from)
            .collect::<Vec<_>>();

        let js_configs = serde_json::to_value(js_configs).expect("Should be serializable");
        let js_configs =
            JsValue::from_json(&js_configs, context).expect("Unable to read JSON value");

        context
            .register_global_property(
                boa_engine::js_string!("serviceConfigs"),
                js_configs,
                Attribute::READONLY,
            )
            .expect("Property registration failed unexpectedly");
    }

    fn parse_service_config<Iter>(
        services: Iter,
        transformed_configs: serde_json::value::Value,
    ) -> Result<Vec<DeployableService>, HooksError>
    where
        Iter: IntoIterator<Item = DeployableService>,
    {
        let mut transformed_configs =
            serde_json::from_value::<Vec<JsServiceConfig>>(transformed_configs).map_err(|err| {
                error!("Cannot parse result of deployment hook: {err}");
                HooksError::InvalidDeploymentHook {
                    err: err.to_string(),
                }
            })?;

        Ok(services
            .into_iter()
            .filter_map(move |service| {
                let index = transformed_configs.iter().position(|transformed_config| {
                    transformed_config.name == service.blueprint_service.service_name
                        && transformed_config.r#type == service.service_type
                        && transformed_config.image == service.blueprint_service.image
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
        service.blueprint_service.files = Some(self.files);

        let env = match service.blueprint_service.env.as_ref().cloned() {
            Some(env) => {
                let mut variables = Vec::new();

                for ev in env.iter() {
                    if let Some(value) = self.env.remove(ev.key()) {
                        variables.push(ev.clone().with_value(value));
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

        service.blueprint_service.env = env;

        service
    }
}

impl From<&DeployableService> for JsServiceConfig {
    fn from(config: &DeployableService) -> Self {
        Self {
            name: config.blueprint_service.service_name.clone(),
            image: config.blueprint_service.image.clone(),
            env: config
                .blueprint_service
                .env
                .as_ref()
                .map(|env| {
                    env.iter()
                        .map(|v| (v.key().clone(), v.value().clone()))
                        .collect()
                })
                .unwrap_or_default(),
            files: config
                .blueprint_service
                .files
                .as_ref()
                .cloned()
                .unwrap_or_default(),
            r#type: config.service_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{app_deployment::AppDeploymentBuilder, blueprint_service};
    use std::io::Write;
    use std::vec;
    use tempfile::NamedTempFile;

    mod apply_deployment_hook {
        use super::*;

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
        async fn with_file_modification() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                    return configs.map((config, index) => {
                        config.files['/etc/some-config.txt'] = config.name + index;
                        return config;
                    });
                }
            "#;

            let (_temp_js_file, config) = config_with_deployment_hook(script);

            let app_name = AppName::master();
            let service_configs = vec![blueprint_service!("service-a")];

            let unit = AppDeploymentBuilder::init(app_name, service_configs, None)
                .finish()
                .unwrap();

            let services = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await
                .unwrap();

            let deployed_files = services
                .iter()
                .filter_map(|service| service.blueprint_service.files.as_ref())
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
        async fn with_file_removal() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                    return configs.map((config, index) => {
                        delete config.files['/etc/some-config.txt'];
                        return config;
                    });
                }
            "#;

            let (_temp_js_file, config) = config_with_deployment_hook(script);

            let app_name = AppName::master();

            let service_config = blueprint_service!("service-a", "some-image", env = (), files = (
                "/etc/some-config.txt" =>  "value"
            ));

            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let services = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await
                .unwrap();

            let deployed_files = services
                .iter()
                .filter_map(|service| service.blueprint_service.files.as_ref())
                .flatten()
                .map(|(path, content)| (path.to_str().unwrap().to_string(), content.clone()))
                .collect::<Vec<(String, SecUtf8)>>();

            assert_eq!(deployed_files, vec![]);

            Ok(())
        }

        #[tokio::test]
        async fn add_env() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                    return configs.map((config, index) => {
                        config.env['VARIABLE_X'] = config.name + index;
                        return config;
                    });
                }
            "#;

            let (_temp_js_file, config) = config_with_deployment_hook(script);
            let app_name = AppName::master();
            let service_config = blueprint_service!("service-a");

            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let services = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await
                .unwrap();

            let deployed_variables = services
                .iter()
                .filter_map(|service| service.blueprint_service.env.as_ref())
                .flat_map(|env| env.iter())
                .map(|env| (env.key().clone(), env.value().unsecure().to_string()))
                .collect::<Vec<(String, String)>>();

            assert_eq!(
                deployed_variables,
                vec![(String::from("VARIABLE_X"), String::from("service-a0"))]
            );

            Ok(())
        }

        #[tokio::test]
        async fn with_env_modification() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                    return configs.map((config, index) => {
                        config.env['VARIABLE_Y'] = config.name + index;
                        return config;
                    });
                }
            "#;

            let (_temp_js_file, config) = config_with_deployment_hook(script);
            let app_name = AppName::master();

            let service_config =
                blueprint_service!("service-a", "some-image", env = ("VARIABLE_X" => "Hello"));

            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let services = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await
                .unwrap();

            let deployed_variables = services
                .iter()
                .filter_map(|service| service.blueprint_service.env.as_ref())
                .flat_map(|env| env.iter())
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
        async fn with_env_removal() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                    return configs.map((config, index) => {
                        delete config.env['VARIABLE_X'];
                        return config;
                    });
                }
            "#;

            let (_temp_js_file, config) = config_with_deployment_hook(script);

            let app_name = AppName::master();
            let service_config =
                blueprint_service!("service-a", "some-image", env = ("VARIABLE_X" => "Hello"));

            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let services = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await
                .unwrap();

            let deployed_variables = services
                .iter()
                .filter_map(|service| service.blueprint_service.env.as_ref())
                .flat_map(|env| env.iter())
                .map(|env| (env.key().clone(), env.value().unsecure().to_string()))
                .collect::<Vec<(String, String)>>();

            assert_eq!(deployed_variables, vec![]);

            Ok(())
        }

        #[tokio::test]
        async fn without_adding_additional_services() -> anyhow::Result<()> {
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
            let service_config = blueprint_service!("service-a");
            let (_temp_js_file, config) = config_with_deployment_hook(script);
            let app_name = AppName::master();

            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let services = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await
                .unwrap();

            let deployed_services = services
                .iter()
                .map(|services| services.blueprint_service.service_name.clone())
                .collect::<Vec<_>>();

            assert_eq!(deployed_services, vec![String::from("service-a")]);

            Ok(())
        }

        #[tokio::test]
        async fn fail_when_modifying_immutable_values() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                    return configs.map((config, index) => {
                        config.name = config.name + index;
                        return config;
                    });
                }
            "#;

            let service_config = blueprint_service!("service-a");
            let (_temp_js_file, config) = config_with_deployment_hook(script);
            let app_name = AppName::master();

            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let services = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await
                .unwrap();

            assert!(services.is_empty());

            Ok(())
        }

        #[tokio::test]
        async fn fail_with_undefined_var_usage() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                    return undefinedVar + appName
                }
            "#;

            let service_config = blueprint_service!("service-a");
            let (_temp_js_file, config) = config_with_deployment_hook(script);
            let app_name = AppName::master();
            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let result = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await;

            assert_eq!(
                result,
                Err(HooksError::InvalidDeploymentHook {
                    err: String::from(
                        "Cannot execute hook: ReferenceError: undefinedVar is not defined\n    at deploymentHook (unknown at :2:61)\n    at <main> (unknown at :1:15)"
                    )
                })
            );

            Ok(())
        }

        #[tokio::test]
        async fn fail_gracefully_if_script_does_not_return_a_value() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                }
            "#;

            let service_config = blueprint_service!("service-a");
            let (_temp_js_file, config) = config_with_deployment_hook(script);
            let app_name = AppName::master();
            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let result = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await;

            assert_eq!(result, Err(HooksError::DeploymentHookReturnedNothing));

            Ok(())
        }

        #[tokio::test]
        async fn fail_with_hook_returning_invalid_object() -> anyhow::Result<()> {
            let script = r#"
                function deploymentHook( appName, configs ) {
                    return 'unexpected return value';
                }
            "#;

            let service_config = blueprint_service!("service-a");
            let (_temp_js_file, config) = config_with_deployment_hook(script);
            let app_name = AppName::master();
            let unit = AppDeploymentBuilder::init(app_name, vec![service_config], None)
                .finish()
                .unwrap();

            let result = Hooks::new(&config)
                .apply_deployment_hook(&AppName::master(), unit.services)
                .await;

            assert!(
                matches!(result, Err(HooksError::InvalidDeploymentHook { err }) if err == "invalid type: string \"unexpected return value\", expected a sequence")
            );

            Ok(())
        }
    }

    mod apply_id_token_claims_to_owner_hook {
        use crate::auth::AdditionalClaims;

        use super::*;
        use openidconnect::{
            EndUserName, IdTokenClaims, IssuerUrl, LocalizedClaim, StandardClaims,
            SubjectIdentifier,
        };

        #[tokio::test]
        async fn fail_with_none_existing_file() {
            let config = crate::config_from_str!(
                r#"
                [hooks]
                idTokenClaimsToOwner = "/path/does/not/exists"
                "#
            );
            let hook = Hooks::new(&config);

            let result = hook
                .apply_id_token_claims_to_owner_hook(User::Oidc {
                    id_token_claims: IdTokenClaims::new(
                        IssuerUrl::new(String::from("https://github.com")).unwrap(),
                        Vec::new(),
                        chrono::Utc::now(),
                        chrono::Utc::now(),
                        StandardClaims::new(SubjectIdentifier::new(String::from("github-user"))),
                        AdditionalClaims::empty(),
                    ),
                })
                .await;

            assert_eq!(
                result,
                Err(HooksError::UserToOwner {
                    err: String::from(
                        "Cannot read hook file \"/path/does/not/exists\": No such file or directory (os error 2)"
                    )
                })
            )
        }

        fn config_with_claims_to_owner_hook(
            script: Option<&str>,
        ) -> (Option<NamedTempFile>, Config) {
            let Some(script) = script else {
                return (None, Config::default());
            };

            let mut hook_file = NamedTempFile::new().unwrap();

            hook_file.write_all(script.as_bytes()).unwrap();

            let config = crate::config_from_str!(&format!(
                r#"
                [hooks]
                idTokenClaimsToOwner = {:?}
                "#,
                hook_file.path()
            ));

            (Some(hook_file), config)
        }

        #[tokio::test]
        async fn for_anonymous_without_hook() -> anyhow::Result<()> {
            let (_temp_js_file, config) = config_with_claims_to_owner_hook(None);
            let hook = Hooks::new(&config);

            let owner = hook
                .apply_id_token_claims_to_owner_hook(User::Anonymous)
                .await?;

            assert_eq!(owner, None);

            Ok(())
        }

        #[tokio::test]
        async fn for_oidc_without_hook() -> anyhow::Result<()> {
            let (_temp_js_file, config) = config_with_claims_to_owner_hook(None);
            let hook = Hooks::new(&config);

            let owner = hook
                .apply_id_token_claims_to_owner_hook(User::Oidc {
                    id_token_claims: IdTokenClaims::new(
                        IssuerUrl::new(String::from("https://github.com")).unwrap(),
                        Vec::new(),
                        chrono::Utc::now(),
                        chrono::Utc::now(),
                        StandardClaims::new(SubjectIdentifier::new(String::from("github-user"))),
                        AdditionalClaims::empty(),
                    ),
                })
                .await?;

            assert_eq!(
                owner,
                Some(Owner {
                    sub: SubjectIdentifier::new(String::from("github-user")),
                    iss: IssuerUrl::new(String::from("https://github.com")).unwrap(),
                    name: None
                })
            );

            Ok(())
        }

        #[tokio::test]
        async fn for_oidc_with_hook() -> anyhow::Result<()> {
            let (_temp_js_file, config) = config_with_claims_to_owner_hook(Some(
                r#"
                function idTokenClaimsToOwnerHook(claims) {
                   return {
                      sub: claims.user_id ? claims.user_id : claims.sub,
                      iss: claims.iss,
                      name: claims.user_name ? claims.user_name : claims.name,
                   };
                }
                "#,
            ));
            let hook = Hooks::new(&config);

            let mut name = LocalizedClaim::new();
            name.insert(None, EndUserName::new(String::from("Some Person")));
            let owner = hook
                .apply_id_token_claims_to_owner_hook(User::Oidc {
                    id_token_claims: IdTokenClaims::new(
                        IssuerUrl::new(String::from("https://github.com")).unwrap(),
                        Vec::new(),
                        chrono::Utc::now(),
                        chrono::Utc::now(),
                        StandardClaims::new(SubjectIdentifier::new(String::from("github-user")))
                            .set_name(Some(name)),
                        AdditionalClaims::with_claims(serde_json::json!({ "user_id": "user-id" })),
                    ),
                })
                .await?;

            assert_eq!(
                owner,
                Some(Owner {
                    sub: SubjectIdentifier::new(String::from("user-id")),
                    iss: IssuerUrl::new(String::from("https://github.com")).unwrap(),
                    name: Some(String::from("Some Person")),
                })
            );

            Ok(())
        }

        #[tokio::test]
        async fn fail_gracefully_if_script_is_malformed() {
            let (_temp_js_file, config) = config_with_claims_to_owner_hook(Some(
                r#"
                function idTokenClaimsToOwnerHook(claims) {
                   return undefinedVar + claims.sub;
                }
                "#,
            ));
            let hook = Hooks::new(&config);

            let mut name = LocalizedClaim::new();
            name.insert(None, EndUserName::new(String::from("Some Person")));
            let result = hook
                .apply_id_token_claims_to_owner_hook(User::Oidc {
                    id_token_claims: IdTokenClaims::new(
                        IssuerUrl::new(String::from("https://github.com")).unwrap(),
                        Vec::new(),
                        chrono::Utc::now(),
                        chrono::Utc::now(),
                        StandardClaims::new(SubjectIdentifier::new(String::from("github-user")))
                            .set_name(Some(name)),
                        AdditionalClaims::with_claims(serde_json::json!({ "user_id": "user-id" })),
                    ),
                })
                .await;

            assert_eq!(
                result,
                Err(HooksError::UserToOwner {
                    err: String::from(
                        "Cannot execute hook: ReferenceError: undefinedVar is not defined\n    at idTokenClaimsToOwnerHook (unknown at :2:59)\n    at <main> (unknown at :1:25)"
                    )
                })
            )
        }

        #[tokio::test]
        async fn fail_gracefully_if_script_does_not_return_a_value() {
            let (_temp_js_file, config) = config_with_claims_to_owner_hook(Some(
                r#"
                function idTokenClaimsToOwnerHook(claims) {
                }
                "#,
            ));
            let hook = Hooks::new(&config);

            let mut name = LocalizedClaim::new();
            name.insert(None, EndUserName::new(String::from("Some Person")));
            let result = hook
                .apply_id_token_claims_to_owner_hook(User::Oidc {
                    id_token_claims: IdTokenClaims::new(
                        IssuerUrl::new(String::from("https://github.com")).unwrap(),
                        Vec::new(),
                        chrono::Utc::now(),
                        chrono::Utc::now(),
                        StandardClaims::new(SubjectIdentifier::new(String::from("github-user")))
                            .set_name(Some(name)),
                        AdditionalClaims::with_claims(serde_json::json!({ "user_id": "user-id" })),
                    ),
                })
                .await;

            assert_eq!(result, Err(HooksError::UserToOwnerReturnedNothing))
        }
    }
}
