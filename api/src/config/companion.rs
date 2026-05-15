/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2020 aixigo AG
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
use crate::config::AppSelector;
use domain::{
    AppName, Image,
    app_blueprints::{Environment, ServiceConfig},
    app_deployment::{
        StaticCompanion, StaticCompanionDeploymentStrategy, StaticCompanionStorageStrategy,
    },
    app_instance::ContainerType,
    templating::TemplateData,
};
use handlebars::{RenderError, RenderErrorReason};
use jsonschema::Validator;
use secstr::SecUtf8;
use serde_value::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::path::PathBuf;

#[derive(Clone, Default, Deserialize)]
pub struct Companions {
    #[serde(default)]
    bootstrapping: Bootstrapping,
    #[serde(flatten)]
    companions: BTreeMap<String, Companion>,
    #[serde(default)]
    templating: Templating,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Companion {
    service_name: Option<String>,
    #[serde(rename = "type")]
    companion_type: CompanionType,
    image: Image,
    #[serde(default)]
    deployment_strategy: DeploymentStrategy,
    env: Option<Environment>,
    #[serde(default)]
    labels: HashMap<String, String>,
    #[serde(alias = "volumes", alias = "files", default)]
    files: Option<BTreeMap<PathBuf, SecUtf8>>,
    #[serde(default = "AppSelector::default")]
    app_selector: AppSelector,
    routing: Option<Routing>,
    #[serde(default)]
    storage_strategy: StorageStrategy,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub(super) enum CompanionType {
    #[serde(rename = "application")]
    Application,
    #[serde(rename = "service")]
    Service,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Default)]
pub enum StorageStrategy {
    #[serde(rename = "none")]
    #[default]
    NoMountVolumes,
    #[serde(rename = "mount-declared-image-volumes")]
    MountDeclaredImageVolumes,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Default)]
pub enum DeploymentStrategy {
    #[serde(rename = "redeploy-always")]
    #[default]
    Always,
    #[serde(rename = "redeploy-on-image-update")]
    OnImageUpdate,
    #[serde(rename = "redeploy-never")]
    Never,
}

/// Helper that configures the service routing for Traefik (see
/// [here](https://docs.traefik.io/routing/routers/)).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Routing {
    pub rule: Option<String>,
    #[serde(default)]
    pub additional_middlewares: BTreeMap<String, Value>,
}

#[derive(Clone, Default, Deserialize)]
struct Bootstrapping {
    containers: Vec<RawBootstrappingContainer>,
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq)]
pub enum ImagePullPolicy {
    #[default]
    Always,
    Never,
    IfNotPresent,
}

impl Display for ImagePullPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImagePullPolicy::Always => f.write_str("Always"),
            ImagePullPolicy::Never => f.write_str("Never"),
            ImagePullPolicy::IfNotPresent => f.write_str("IfNotPresent"),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawBootstrappingContainer {
    image: String,
    #[serde(default)]
    image_pull_policy: ImagePullPolicy,
    #[serde(default)]
    args: Vec<String>,
}

pub struct BootstrappingContainer {
    pub image: Image,
    pub image_pull_policy: ImagePullPolicy,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Templating {
    user_defined_schema: Option<serde_json::Value>,
}

impl<'de> serde::Deserialize<'de> for Templating {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        let user_defined_schema = match value {
            serde_json::Value::Object(mut obj) => match obj.remove("userDefinedSchema") {
                None => None,
                Some(user_defined_schema) => {
                    if let Err(err) = Validator::new(&user_defined_schema) {
                        return Err(serde::de::Error::custom(format!(
                            "Invalid user defined schema: {err}"
                        )));
                    }

                    Some(user_defined_schema)
                }
            },
            _ => None,
        };

        Ok(Self {
            user_defined_schema,
        })
    }
}

impl Companions {
    pub fn to_static_companions(&self, app_name: &AppName) -> Vec<StaticCompanion> {
        self.companions
            .iter()
            .filter(|(_, companion)| companion.matches_app_name(app_name))
            .map(|(companion_name, companion)| Self::create_companion(companion_name, companion))
            .collect()
    }

    fn create_companion(companion_name: &str, companion: &Companion) -> StaticCompanion {
        let blueprint_config = ServiceConfig {
            service_name: companion
                .service_name
                .as_deref()
                .unwrap_or(companion_name)
                .to_string(),
            image: companion.image.clone(),
            env: companion.env.as_ref().and_then(|env| {
                if env.iter().count() == 0 {
                    return None;
                }

                Some(Environment::new(
                    env.iter()
                        .map(|variable| variable.clone().with_templated(true))
                        .collect(),
                ))
            }),
            files: companion.files.clone(),
        };

        let rule_template = companion.routing.as_ref().and_then(|r| r.rule.clone());
        let middleware_templates = companion
            .routing
            .as_ref()
            .map(|r| r.additional_middlewares.clone());

        let deployment_strategy = match companion.deployment_strategy {
            DeploymentStrategy::Always => StaticCompanionDeploymentStrategy::Always,
            DeploymentStrategy::OnImageUpdate => StaticCompanionDeploymentStrategy::OnImageUpdate,
            DeploymentStrategy::Never => StaticCompanionDeploymentStrategy::Never,
        };

        let storage_strategy = match companion.storage_strategy {
            StorageStrategy::NoMountVolumes => StaticCompanionStorageStrategy::NoMountVolumes,
            StorageStrategy::MountDeclaredImageVolumes => {
                StaticCompanionStorageStrategy::MountDeclaredImageVolumes
            }
        };

        let labels = companion.labels.clone();

        let companion = match companion.companion_type {
            CompanionType::Application => StaticCompanion::app_companion(blueprint_config),
            CompanionType::Service => StaticCompanion::service_companion(blueprint_config),
        };

        companion
            .with_labels(labels)
            .with_deployment_strategy(deployment_strategy)
            .with_templated_rule(rule_template)
            .with_templated_middlewares(middleware_templates)
            .with_storage_strategy(storage_strategy)
    }

    pub(super) fn user_defined_schema_validator(&self) -> Option<Validator> {
        let schema = self.templating.user_defined_schema.as_ref()?;
        Validator::new(schema).ok()
    }

    /// Applies templating to all bootstrapping containers and returns the templated set of
    /// containers..
    pub fn companion_bootstrapping_containers(
        &self,
        template_data: &TemplateData,
    ) -> Result<Vec<BootstrappingContainer>, RenderError> {
        let handlebars = template_data.as_handlerbars();

        let mut containers = Vec::with_capacity(self.bootstrapping.containers.len());
        for c in self.bootstrapping.containers.iter() {
            let img = handlebars.render(&c.image)?;

            let mut args = Vec::with_capacity(c.args.len());
            for arg in c.args.iter() {
                args.push(handlebars.render(arg)?);
            }

            containers.push(BootstrappingContainer {
                image: img
                    .parse::<Image>()
                    .map_err(|e| RenderErrorReason::Other(e.to_string()))?,
                image_pull_policy: c.image_pull_policy.clone(),
                args,
            });
        }

        Ok(containers)
    }
}

impl Companion {
    pub fn matches_app_name(&self, app_name: &AppName) -> bool {
        self.app_selector.matches(app_name)
    }
}

impl From<CompanionType> for ContainerType {
    fn from(t: CompanionType) -> Self {
        match t {
            CompanionType::Application => ContainerType::ApplicationCompanion,
            CompanionType::Service => ContainerType::ServiceCompanion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_from_str;
    use domain::blueprint_service;
    use pretty_assertions::assert_eq;
    use std::{collections::HashMap, str::FromStr};
    use url::Url;

    macro_rules! companion_from_str {
        ( $config_str:expr_2021 ) => {
            toml::de::from_str::<Companion>($config_str).unwrap()
        };
    }

    macro_rules! companions_from_str {
        ( $config_str:expr_2021 ) => {
            toml::de::from_str::<Companions>($config_str).unwrap()
        };
    }

    #[test]
    fn to_static_companions() {
        let config = config_from_str!(
            r#"
            [companions.openid]
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

        let companion_configs = config.companions.to_static_companions(&AppName::master());

        assert_eq!(
            companion_configs,
            vec![
                StaticCompanion::service_companion(blueprint_service!(
                    "{{service-name}}-nginx",
                    "nginx:latest",
                    templated_env = ("KEY" => "VALUE")
                )),
                StaticCompanion::app_companion(blueprint_service!(
                    "openid",
                    "private.example.com/library/openid:latest",
                    templated_env = ("KEY" => "VALUE")
                )),
            ]
        );
    }

    #[test]
    fn to_static_companions_with_deployment_strategy() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'service'
            image = 'private.example.com/library/openid:latest'
            deploymentStrategy = 'redeploy-on-image-update'
            "#
        );

        let companion_configs = config.companions.to_static_companions(&AppName::master());

        assert_eq!(
            companion_configs,
            vec![
                StaticCompanion::service_companion(blueprint_service!(
                    "openid",
                    "private.example.com/library/openid:latest"
                ))
                .with_deployment_strategy(StaticCompanionDeploymentStrategy::OnImageUpdate)
            ]
        );
    }

    #[test]
    fn to_static_companions_with_files() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:11-alpine'

            [companions.openid.volumes]
            '/tmp/test-1.json' = '{}'
            '/tmp/test-2.json' = '{}'
            "#
        );

        let companion_configs = config.companions.to_static_companions(&AppName::master());

        assert_eq!(
            companion_configs,
            vec![StaticCompanion::app_companion(blueprint_service!(
                "openid",
                "private.example.com/library/openid:11-alpine",
                files = (
                    "/tmp/test-1.json" => "{}",
                    "/tmp/test-2.json" => "{}"
                )
            ))]
        );
    }

    #[test]
    fn to_static_companions_with_with_labels() {
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

        let companion_configs = config.companions.to_static_companions(&AppName::master());

        assert_eq!(
            companion_configs,
            vec![
                StaticCompanion::app_companion(blueprint_service!(
                    "openid",
                    "private.example.com/library/openid:11-alpine"
                ))
                .with_labels(HashMap::from([(
                    String::from("com.example.foo"),
                    String::from("bar")
                )]))
            ]
        );
    }

    #[rstest::rstest]
    #[case::without_service_name_override(
        r#"
            [companions.openid]
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ 'KEY=VALUE' ]
            appSelector = "master"
        "#
    )]
    #[case::with_service_name_override(
        r#"
            [companions.openid_some_key]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
            env = [ 'KEY=VALUE' ]
            appSelector = "master"
        "#
    )]
    fn to_static_companions_with_app_name_selection(#[case] config: &str) {
        let config = config_from_str!(config);

        let companion_configs = config.companions.to_static_companions(&AppName::master());

        assert_eq!(
            companion_configs,
            vec![StaticCompanion::app_companion(blueprint_service!(
                "openid",
                "private.example.com/library/openid:latest",
                env = ("KEY" => "VALUE")
            ))]
        );
        assert_eq!(
            config
                .companions
                .to_static_companions(&AppName::from_str("other").unwrap()),
            Vec::new()
        );
    }

    #[test]
    fn to_static_companions_with_storage_strategy() {
        let config = config_from_str!(
            r#"
            [companions.openid]
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:11-alpine'
            storageStrategy = 'mount-declared-image-volumes'
            "#
        );

        let companion_configs = config.companions.to_static_companions(&AppName::master());

        assert_eq!(
            companion_configs,
            vec![
                StaticCompanion::app_companion(blueprint_service!(
                    "openid",
                    "private.example.com/library/openid:11-alpine"
                ))
                .with_storage_strategy(StaticCompanionStorageStrategy::MountDeclaredImageVolumes),
            ]
        );
    }

    #[test]
    fn parse_companion_with_required_fields_and_optionanl_fields() {
        let companion = companion_from_str!(
            r#"
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
        "#
        );

        assert_eq!(companion.service_name, Some(String::from("openid")));
        assert_eq!(companion.companion_type, CompanionType::Application);
        assert_eq!(
            companion.image,
            Image::from_str("private.example.com/library/openid:latest").unwrap()
        );
        assert_eq!(companion.deployment_strategy, DeploymentStrategy::Always);
    }

    #[test]
    fn parse_companion_with_router_rule() {
        let companion = companion_from_str!(
            r#"
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'

            [routing]
            rule = 'PathPrefix(`/{{application.name}}/adminer/sub-path`)'
        "#
        );

        assert_eq!(
            companion.routing,
            Some(Routing {
                rule: Some(String::from(
                    "PathPrefix(`/{{application.name}}/adminer/sub-path`)"
                )),
                additional_middlewares: BTreeMap::new(),
            })
        )
    }

    #[test]
    fn parse_companion_with_additional_middlewares() {
        let companion = companion_from_str!(
            r#"
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'

            [routing.additionalMiddlewares]
            stripPrefixes = { 'prefixes' = ['/{{application.name}}/'] }
        "#
        );

        assert_eq!(
            companion.routing,
            Some(Routing {
                rule: None,
                additional_middlewares: BTreeMap::from([(
                    String::from("stripPrefixes"),
                    serde_value::to_value(serde_json::json!({
                        "prefixes": [ "/{{application.name}}/" ]
                    }))
                    .unwrap()
                )]),
            })
        )
    }

    #[test]
    fn should_parse_companion_bootstrap_containers() {
        let companions = companions_from_str!(
            r#"
            [[bootstrapping.containers]]
            image = "busybox"
            imagePullPolicy = "Never"
            "#
        );

        let container = &companions.bootstrapping.containers[0];

        assert_eq!(container.image, String::from("busybox"));
        assert_eq!(container.image_pull_policy, ImagePullPolicy::Never);
        assert_eq!(container.args, Vec::<String>::new());
    }

    #[test]
    fn should_parse_companion_bootstrap_containers_and_template_args() {
        let companions = companions_from_str!(
            r#"
            [[bootstrapping.containers]]
            image = "busybox"
            args = [ "echo", "Hello {{application.name}}" ]
            "#
        );

        let containers = &companions
            .companion_bootstrapping_containers(&TemplateData {
                application: domain::templating::ApplicationTemplateData {
                    name: &AppName::master(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();

        assert_eq!(containers[0].image, Image::from_str("busybox").unwrap());
        assert_eq!(containers[0].image_pull_policy, ImagePullPolicy::Always);
        assert_eq!(
            containers[0].args,
            vec![String::from("echo"), String::from("Hello master")]
        );
    }

    #[test]
    fn should_parse_companion_bootstrap_containers_and_template_url_args() {
        let companions = companions_from_str!(
            r#"
            [[bootstrapping.containers]]
            image = "busybox"
            args = [ "echo", "Hello {{application.baseUrl}}" ]
            "#
        );

        let containers = &companions
            .companion_bootstrapping_containers(&TemplateData {
                application: domain::templating::ApplicationTemplateData {
                    base_url: Url::parse("http://example.com").ok().as_ref(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();

        assert_eq!(containers[0].image, Image::from_str("busybox").unwrap());
        assert_eq!(
            containers[0].args,
            vec![
                String::from("echo"),
                String::from("Hello http://example.com/")
            ]
        );
    }

    #[test]
    fn should_parse_companion_bootstrap_containers_and_template_infrastructure_information() {
        let companions = companions_from_str!(
            r#"
            [[bootstrapping.containers]]
            image = "busybox"
            args = [ "echo", "Hello {{infrastructure.namespace}}" ]
            "#
        );

        let containers = &companions
            .companion_bootstrapping_containers(&TemplateData {
                application: domain::templating::ApplicationTemplateData {
                    ..Default::default()
                },
                infrastructure: Some(&serde_json::json!({
                    "namespace": "my-namespace"
                })),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(containers[0].image, Image::from_str("busybox").unwrap());
        assert_eq!(
            containers[0].args,
            vec![String::from("echo"), String::from("Hello my-namespace")]
        );
    }

    #[test]
    fn should_parse_companion_bootstrap_containers_with_templated_image() {
        let companions = companions_from_str!(
            r#"
            [[bootstrapping.containers]]
            image = """busybox{{#if (eq application.name "master")}}:v0{{/if}}"""
            "#
        );

        let containers = &companions
            .companion_bootstrapping_containers(&TemplateData {
                application: domain::templating::ApplicationTemplateData {
                    name: &AppName::master(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();

        assert_eq!(containers[0].image, Image::from_str("busybox:v0").unwrap());
    }

    #[test]
    fn should_parse_companion_bootstrap_containers_with_templated_user_defined_parameters_image() {
        let companions = companions_from_str!(
            r#"
            [[bootstrapping.containers]]
            image = """busybox:{{userDefined}}"""
            "#
        );

        let containers = &companions
            .companion_bootstrapping_containers(&TemplateData {
                user_defined_parameters: Some(&serde_json::json!("v0")),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(containers[0].image, Image::from_str("busybox:v0").unwrap());
    }

    #[test]
    fn should_parse_user_defined_templating_schema() {
        let companions = companions_from_str!(
            r#"
            [templating.userDefinedSchema]
            type = "string"
        "#
        );

        let validator = companions.user_defined_schema_validator().unwrap();

        assert!(validator.is_valid(&serde_json::json!("test")));
    }

    #[test]
    fn should_not_parse_user_defined_templating_with_invalid_schema() {
        use figment::providers::Format;
        let provider = figment::providers::Toml::string(
            r#"
            [companions.templating.userDefinedSchema]
            type = "i-am-a-teapot"
        "#,
        );
        let config = figment::Figment::from(provider).extract::<crate::config::Config>();

        assert!(matches!(
            config,
            Err(figment::Error {
                kind, ..
            }) if kind == figment::error::Kind::Message(String::from("Invalid user defined schema: \"i-am-a-teapot\" is not valid under any of the schemas listed in the 'anyOf' keyword"))
        ));
    }
}
