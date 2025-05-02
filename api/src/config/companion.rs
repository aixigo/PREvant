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
use crate::models::service::ContainerType;
use crate::models::user_defined_parameters::UserDefinedParameters;
use crate::models::{AppName, Environment, Image, ServiceConfig};
use handlebars::{Handlebars, RenderError, RenderErrorReason};
use jsonschema::Validator;
use secstr::SecUtf8;
use serde_value::Value;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::PathBuf;
use url::Url;

#[derive(Clone, Default, Deserialize)]
pub(super) struct Companions {
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
    service_name: String,
    #[serde(rename = "type")]
    companion_type: CompanionType,
    image: Image,
    #[serde(default)]
    deployment_strategy: DeploymentStrategy,
    env: Option<Environment>,
    labels: Option<BTreeMap<String, String>>,
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

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub enum StorageStrategy {
    #[serde(rename = "none")]
    NoMountVolumes,
    #[serde(rename = "mount-declared-image-volumes")]
    MountDeclaredImageVolumes,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub enum DeploymentStrategy {
    #[serde(rename = "redeploy-always")]
    RedeployAlways,
    #[serde(rename = "redeploy-on-image-update")]
    RedeployOnImageUpdate,
    #[serde(rename = "redeploy-never")]
    RedeployNever,
}

/// Helper that configures the service routing for Traefik (see
/// [here](https://docs.traefik.io/routing/routers/)).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Routing {
    pub rule: Option<String>,
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
    pub(super) fn companion_configs<P>(
        &self,
        app_name: &AppName,
        predicate: P,
    ) -> Vec<(ServiceConfig, DeploymentStrategy, StorageStrategy)>
    where
        P: Fn(&Companion) -> bool,
    {
        self.companions
            .iter()
            .filter(|(_, companion)| companion.matches_app_name(app_name))
            .filter(|(_, companion)| predicate(companion))
            .map(|(_, companion)| {
                (
                    ServiceConfig::from(companion.clone()),
                    companion.deployment_strategy().clone(),
                    companion.storage_strategy().clone(),
                )
            })
            .collect()
    }

    pub(super) fn user_defined_schema_validator(&self) -> Option<Validator> {
        let schema = self.templating.user_defined_schema.as_ref()?;
        Validator::new(schema).ok()
    }

    /// Applies templating to all bootstrapping containers and returns the templated set of
    /// containers..
    ///
    /// * `infrastructure` - Additional parameter that holds infrastructure specific information
    ///   such [Kubernetes namespace](https://kubernetes.io/docs/concepts/overview/working-with-objects/namespaces/)
    pub(super) fn companion_bootstrapping_containers<S>(
        &self,
        app_name: &AppName,
        base_url: &Option<Url>,
        infrastructure: Option<S>,
        user_defined_parameters: &Option<UserDefinedParameters>,
    ) -> Result<Vec<BootstrappingContainer>, RenderError>
    where
        S: serde::Serialize,
    {
        let handlebars = Handlebars::new();

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct AppData<'a> {
            name: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            base_url: &'a Option<Url>,
        }

        // TODO: apply same pattern as for companions. {{application.name}}, {{service.…}}…
        #[derive(Serialize)]
        struct Data<'a, S> {
            application: AppData<'a>,
            #[serde(skip_serializing_if = "Option::is_none")]
            infrastructure: Option<S>,
            #[serde(skip_serializing_if = "Option::is_none", rename = "userDefined")]
            user_defined_parameters: &'a Option<UserDefinedParameters>,
        }

        let data = Data {
            infrastructure,
            application: AppData {
                name: app_name,
                base_url,
            },
            user_defined_parameters,
        };

        let mut containers = Vec::with_capacity(self.bootstrapping.containers.len());
        for c in self.bootstrapping.containers.iter() {
            let img = handlebars.render_template(&c.image, &data)?;

            let mut args = Vec::with_capacity(c.args.len());
            for arg in c.args.iter() {
                args.push(handlebars.render_template(arg, &data)?);
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
    pub fn companion_type(&self) -> &CompanionType {
        &self.companion_type
    }

    pub fn matches_app_name(&self, app_name: &AppName) -> bool {
        self.app_selector.matches(app_name)
    }

    pub fn deployment_strategy(&self) -> &DeploymentStrategy {
        &self.deployment_strategy
    }

    pub fn storage_strategy(&self) -> &StorageStrategy {
        &self.storage_strategy
    }
}

// TODO: this From implementation and companion_configs provides a circular dependency between
// config and ServiceConfig
impl From<Companion> for ServiceConfig {
    fn from(companion: Companion) -> ServiceConfig {
        let mut config =
            ServiceConfig::new(companion.service_name.clone(), companion.image.clone());

        config.set_env(companion.env.clone().map(|env| {
            Environment::new(
                env.iter()
                    .map(|variable| variable.clone().with_templated(true))
                    .collect(),
            )
        }));
        config.set_labels(companion.labels.clone());

        if let Some(files) = &companion.files {
            config.set_files(Some(files.clone()));
        }

        if let Some(routing) = &companion.routing {
            config.set_routing(routing.clone());
        }

        config.set_container_type(companion.companion_type.into());

        config
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

impl Default for DeploymentStrategy {
    fn default() -> Self {
        Self::RedeployAlways
    }
}

impl Default for StorageStrategy {
    fn default() -> Self {
        Self::NoMountVolumes
    }
}

#[cfg(test)]
mod tests {
    use jsonschema::Validator;

    use super::*;
    use std::str::FromStr;

    macro_rules! companion_from_str {
        ( $config_str:expr ) => {
            toml::de::from_str::<Companion>($config_str).unwrap()
        };
    }

    macro_rules! companions_from_str {
        ( $config_str:expr ) => {
            toml::de::from_str::<Companions>($config_str).unwrap()
        };
    }

    #[test]
    fn should_parse_companion_with_required_fields() {
        let companion = companion_from_str!(
            r#"
            serviceName = 'openid'
            type = 'application'
            image = 'private.example.com/library/openid:latest'
        "#
        );

        assert_eq!(&companion.service_name, "openid");
        assert_eq!(companion.companion_type, CompanionType::Application);
        assert_eq!(
            companion.image,
            Image::from_str("private.example.com/library/openid:latest").unwrap()
        );
        assert_eq!(
            companion.deployment_strategy,
            DeploymentStrategy::RedeployAlways
        );
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
            .companion_bootstrapping_containers(&AppName::master(), &None, None::<String>, &None)
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
            .companion_bootstrapping_containers(
                &AppName::master(),
                &Url::parse("http://example.com").ok(),
                None::<String>,
                &None,
            )
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
            .companion_bootstrapping_containers(
                &AppName::master(),
                &None,
                Some(serde_json::json!({
                    "namespace": "my-namespace"
                })),
                &None,
            )
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
            .companion_bootstrapping_containers(&AppName::master(), &None, None::<String>, &None)
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
            .companion_bootstrapping_containers(
                &AppName::master(),
                &None,
                None::<String>,
                &UserDefinedParameters::new(
                    serde_json::json!("v0"),
                    &Validator::new(&serde_json::json!({"type": "string"})).unwrap(),
                )
                .ok(),
            )
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
