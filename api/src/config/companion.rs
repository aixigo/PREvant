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
use crate::models::{Environment, Image, Router, ServiceConfig};
use secstr::SecUtf8;
use serde_value::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

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
    router: Option<Router>,
    middlewares: Option<BTreeMap<String, Value>>,
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

impl Companion {
    pub fn companion_type(&self) -> &CompanionType {
        &self.companion_type
    }

    pub fn matches_app_name(&self, app_name: &str) -> bool {
        self.app_selector.matches(app_name)
    }

    pub fn deployment_strategy(&self) -> &DeploymentStrategy {
        &self.deployment_strategy
    }

    pub fn storage_strategy(&self) -> &StorageStrategy {
        &self.storage_strategy
    }
}

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

        if let Some(router) = &companion.router {
            config.set_router(router.clone());
        }

        if let Some(middlewares) = &companion.middlewares {
            config.set_middlewares(middlewares.clone());
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
    use super::*;
    use std::str::FromStr;

    macro_rules! companion_from_str {
        ( $config_str:expr ) => {
            toml::de::from_str::<Companion>($config_str).unwrap()
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
}
