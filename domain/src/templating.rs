//! Provides some helper structs and traits to apply templating via [`handlebars`] crate.
//!
//! # Examples
//!
//! The following example demonstrates how the templating can be used to generate a Nginx config
//! file based on dynamic set of services.
//!
//! ```rust
//! use domain::{Image, templating::*, app_blueprints::*, app_instance::ContainerType};
//! use secstr::SecUtf8;
//! use std::{collections::BTreeMap, str::FromStr, path::PathBuf};
//!
//! // create the service config that shall be used for templating
//! let mount_path = PathBuf::from("/etc/ningx/conf.d/default.conf");
//! let mut service_config = ServiceConfig::new(
//!     String::from("nginx-proxy"),
//!     Image::from_str("nginx").unwrap()
//! );
//! service_config.files = Some(BTreeMap::from([
//!     (
//!         mount_path.clone(),
//!         SecUtf8::from(
//!                 r#"{{#each services}}
//! {{~#isNotCompanion type}}
//! location /{{name}} {
//!     proxy_pass http://{{~name~}}:{{~port~}};
//! }
//! {{/isNotCompanion}}
//! {{/each}}"#,
//!         )
//!     )
//! ]));
//!
//! // building some template data (preventing here that Wordpress and Nextcloud will be deployed
//! // as instances)
//! let wordpress_image = Image::from_str("wordpress").unwrap();
//! let nextcloud_image = Image::from_str("nextcloud").unwrap();
//! let template_data = TemplateData {
//!     application: ApplicationTemplateData {
//!         name: "some-app-name",
//!         ..Default::default()
//!     },
//!     service_or_services: ServiceOrServices::Services {
//!         services: vec![ServiceTemplateData {
//!            name: "wordpress",
//!            image: &wordpress_image,
//!            port: 80,
//!            container_type: &ContainerType::Instance,
//!         },
//!         ServiceTemplateData {
//!            name: "nextcloud",
//!            image: &nextcloud_image,
//!            port: 80,
//!            container_type: &ContainerType::Instance,
//!         }],
//!     },
//!     ..Default::default()
//! };
//!
//! // apply the templating
//! let service_config = service_config.templated_clone(&template_data).unwrap();
//!
//! # assert_eq!(
//! #     service_config.files.unwrap().remove(&mount_path).map(|file| file.into_unsecure()),
//! #     Some(String::from(
//! #                 r#"location /wordpress {
//! #     proxy_pass http://wordpress:80;
//! # }
//! # location /nextcloud {
//! #     proxy_pass http://nextcloud:80;
//! # }
//! # "#
//! #     ))
//! # );
//! ```
//!
//! Please note that the example above makes use of [`handlebars_helper::is_not_companion`] which
//! is one of the helper functions to build PREvant templates.

use crate::{Image, app_instance::ContainerType};
use handlebars::{Handlebars, RenderError};
use serde::Serialize;
use url::Url;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationTemplateData<'a> {
    pub name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<&'a Url>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", untagged)]
pub enum ServiceOrServices<'a> {
    Service {
        service: ServiceTemplateData<'a>,
    },
    Services {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        services: Vec<ServiceTemplateData<'a>>,
    },
}
impl<'a> Default for ServiceOrServices<'a> {
    fn default() -> Self {
        Self::Services {
            services: Vec::new(),
        }
    }
}

#[derive(Serialize)]
pub struct ServiceTemplateData<'a> {
    pub name: &'a str,
    pub image: &'a Image,
    pub port: u16,
    #[serde(rename = "type")]
    pub container_type: &'a ContainerType,
}

#[derive(Default, Serialize)]
pub struct TemplateData<'a> {
    pub application: ApplicationTemplateData<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub infrastructure: Option<&'a serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "userDefined")]
    pub user_defined_parameters: Option<&'a serde_json::Value>,
    #[serde(flatten)]
    pub service_or_services: ServiceOrServices<'a>,
}

#[derive(thiserror::Error, Debug)]
pub enum TemplatedCloneError<E> {
    #[error("invalid template string: {0}")]
    RenderError(RenderError),
    #[error("{0}")]
    Other(E),
}

impl<E> From<TemplatedError<E>> for TemplatedCloneError<E> {
    fn from(error: TemplatedError<E>) -> Self {
        match error {
            TemplatedError::RenderError(render_error) => {
                TemplatedCloneError::RenderError(render_error)
            }
            TemplatedError::Other(e) => TemplatedCloneError::Other(e),
        }
    }
}
impl<E> From<RenderError> for TemplatedCloneError<E> {
    fn from(error: RenderError) -> Self {
        Self::RenderError(error)
    }
}

pub trait TemplatedClone<E>: Sized {
    fn templated_clone(&self, template_data: &TemplateData)
    -> Result<Self, TemplatedCloneError<E>>;
}

#[derive(thiserror::Error, Debug)]
pub enum TemplatedError<E> {
    #[error("invalid template string: {0}")]
    RenderError(RenderError),
    #[error("{0}")]
    Other(E),
}

impl<E> From<RenderError> for TemplatedError<E> {
    fn from(error: RenderError) -> Self {
        Self::RenderError(error)
    }
}

pub trait Templated<E>: Sized {
    fn apply_template(self, template_data: &TemplateData) -> Result<Self, TemplatedError<E>>;
}

impl<'b, 'a: 'b> TemplateData<'a> {
    pub fn as_handlerbars(&'a self) -> HandlebarsWrapper<'b> {
        let mut handlebars = Handlebars::new();
        handlebars.register_helper("isCompanion", Box::new(handlebars_helper::is_companion));
        handlebars.register_helper(
            "isNotCompanion",
            Box::new(handlebars_helper::is_not_companion),
        );
        HandlebarsWrapper {
            handlebars,
            data: self,
        }
    }
}

pub struct HandlebarsWrapper<'a> {
    handlebars: Handlebars<'a>,
    data: &'a TemplateData<'a>,
}

impl<'a> HandlebarsWrapper<'a> {
    pub fn render(&self, template_str: &str) -> Result<String, RenderError> {
        self.handlebars.render_template(template_str, self.data)
    }

    /// Walks all items in the [`serde_value::Value`] and treats each [`serde_value::Value::String`]
    /// as a template string
    pub fn render_serde_value(
        &self,
        value: &serde_value::Value,
    ) -> Result<serde_value::Value, RenderError> {
        self.apply_templating_to_middleware_value(&value)
    }

    fn apply_templating_to_middleware_value(
        &self,
        value: &serde_value::Value,
    ) -> Result<serde_value::Value, RenderError> {
        match value {
            serde_value::Value::String(v) => Ok(serde_value::Value::String(self.render(&v)?)),
            serde_value::Value::Seq(values) => {
                let mut templated_values = Vec::with_capacity(values.len());
                for v in values.iter() {
                    templated_values.push(self.apply_templating_to_middleware_value(v)?);
                }
                Ok(serde_value::Value::Seq(templated_values))
            }
            serde_value::Value::Map(map) => {
                let mut templated_map = std::collections::BTreeMap::new();
                for (k, v) in map.iter() {
                    templated_map.insert(k.clone(), self.apply_templating_to_middleware_value(v)?);
                }
                Ok(serde_value::Value::Map(templated_map))
            }
            v => Ok(v.clone()),
        }
    }
}

/// Provides [`handlebars::Helper`] that can be integrated into [`HandlebarsWrapper`].
pub mod handlebars_helper {
    use crate::app_instance::ContainerType;
    use handlebars::{
        Context, Handlebars, Helper, HelperResult, Output, RenderContext, RenderErrorReason,
        Renderable,
    };
    use std::str::FromStr;

    pub fn is_companion<'reg, 'rc>(
        h: &Helper<'rc>,
        r: &'reg Handlebars,
        ctx: &'rc Context,
        rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        let s = h
            .param(0)
            .map(|v| v.value())
            .and_then(|v| v.as_str())
            .ok_or(RenderErrorReason::ParamNotFoundForIndex(
                "parameter type is required",
                0,
            ))?;

        let container_type = ContainerType::from_str(s)
            .map_err(|e| RenderErrorReason::Other(format!("Invalid type paramter {s:?}. {e}")))?;

        match container_type {
            ContainerType::ServiceCompanion | ContainerType::ApplicationCompanion => h
                .template()
                .map(|t| t.render(r, ctx, rc, out))
                .unwrap_or(Ok(())),
            _ => h
                .inverse()
                .map(|t| t.render(r, ctx, rc, out))
                .unwrap_or(Ok(())),
        }
    }

    pub fn is_not_companion<'reg, 'rc>(
        h: &Helper<'rc>,
        r: &'reg Handlebars,
        ctx: &'rc Context,
        rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        let s = h
            .param(0)
            .map(|v| v.value())
            .and_then(|v| v.as_str())
            .ok_or(RenderErrorReason::ParamNotFoundForIndex(
                "parameter type is required",
                0,
            ))?;

        let container_type = ContainerType::from_str(s)
            .map_err(|e| RenderErrorReason::Other(format!("Invalid type parameter {s:?}. {e}")))?;

        match container_type {
            ContainerType::ServiceCompanion | ContainerType::ApplicationCompanion => h
                .inverse()
                .map(|t| t.render(r, ctx, rc, out))
                .unwrap_or(Ok(())),
            _ => h
                .template()
                .map(|t| t.render(r, ctx, rc, out))
                .unwrap_or(Ok(())),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::AppName;
    use anyhow::Result;

    use super::*;
    use assert_json_diff::assert_json_eq;
    use std::str::FromStr;

    #[test]
    fn serialize_with_one_service() {
        let data = TemplateData {
            application: ApplicationTemplateData {
                name: "app-name",
                ..Default::default()
            },
            service_or_services: ServiceOrServices::Service {
                service: ServiceTemplateData {
                    name: "service-name",
                    image: &Image::from_str("service-image").unwrap(),
                    port: 80,
                    container_type: &ContainerType::Instance,
                },
            },
            ..Default::default()
        };

        assert_json_eq!(
            serde_json::to_value(&data).unwrap(),
            serde_json::json!({
                "application": {
                    "name": "app-name"
                },
                "service": {
                    "name": "service-name",
                    "image": "docker.io/library/service-image:latest",
                    "port": 80,
                    "type": "instance",
                }
            })
        );
    }

    #[test]
    fn serialize_with_many_services() {
        let img = Image::from_str("service-image").unwrap();
        let data = TemplateData {
            application: ApplicationTemplateData {
                name: "app-name",
                ..Default::default()
            },
            service_or_services: ServiceOrServices::Services {
                services: vec![ServiceTemplateData {
                    name: "service-name",
                    image: &img,
                    port: 80,
                    container_type: &ContainerType::Instance,
                }],
            },
            ..Default::default()
        };

        assert_json_eq!(
            serde_json::to_value(&data).unwrap(),
            serde_json::json!({
                "application": {
                    "name": "app-name"
                },
                "services": [{
                    "name": "service-name",
                    "image": "docker.io/library/service-image:latest",
                    "port": 80,
                    "type": "instance",
                }]
            })
        );
    }

    #[test]
    fn serialize_without_any_services() {
        let data = TemplateData {
            application: ApplicationTemplateData {
                name: "app-name",
                ..Default::default()
            },
            ..Default::default()
        };

        assert_json_eq!(
            serde_json::to_value(&data).unwrap(),
            serde_json::json!({
                "application": {
                    "name": "app-name"
                },
            })
        );
    }

    #[test]
    fn build_from_template_data() -> Result<()> {
        let data = TemplateData {
            application: ApplicationTemplateData {
                name: "app-name",
                ..Default::default()
            },
            service_or_services: ServiceOrServices::Service {
                service: ServiceTemplateData {
                    name: "service-name",
                    image: &Image::from_str("service-image").unwrap(),
                    port: 80,
                    container_type: &ContainerType::Instance,
                },
            },
            ..Default::default()
        };

        let app_name = AppName::from_str(&data.as_handlerbars().render("{{application.name}}")?)?;
        assert_eq!(app_name.as_str(), "app-name");

        Ok(())
    }
}
