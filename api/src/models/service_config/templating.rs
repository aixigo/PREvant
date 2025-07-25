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

use crate::config::Routing;
use crate::models::user_defined_parameters::UserDefinedParameters;
use crate::models::{AppName, ContainerType, Environment, EnvironmentVariable, ServiceConfig};
use handlebars::{
    Context, Handlebars, Helper, HelperResult, Output, RenderContext, RenderError,
    RenderErrorReason, Renderable,
};
use secstr::SecUtf8;
use serde_value::Value;
use std::collections::BTreeMap;
use std::str::FromStr;
use url::Url;

impl ServiceConfig {
    pub fn apply_templating(
        &self,
        app_name: &AppName,
        base_url: &Option<Url>,
        user_defined_parameters: &Option<UserDefinedParameters>,
    ) -> Result<Self, RenderError> {
        let parameters = TemplateParameters {
            application: ApplicationTemplateParameter {
                name: app_name,
                base_url,
            },
            services: None,
            service: Some(ServiceTemplateParameter {
                name: self.service_name(),
                container_type: self.container_type().clone(),
                port: self.port(),
            }),
            user_defined_parameters,
        };

        self.apply_template(&parameters)
    }

    pub fn apply_templating_for_service_companion(
        &self,
        app_name: &AppName,
        base_url: &Option<Url>,
        service_config: &Self,
        user_defined_parameters: &Option<UserDefinedParameters>,
    ) -> Result<Self, RenderError> {
        let parameters = TemplateParameters {
            application: ApplicationTemplateParameter {
                name: app_name,
                base_url,
            },
            services: None,
            service: Some(ServiceTemplateParameter {
                name: service_config.service_name(),
                container_type: service_config.container_type().clone(),
                port: service_config.port(),
            }),
            user_defined_parameters,
        };

        self.apply_template(&parameters)
    }

    pub fn apply_templating_for_application_companion(
        &self,
        app_name: &AppName,
        base_url: &Option<Url>,
        service_configs: &[Self],
        user_defined_parameters: &Option<UserDefinedParameters>,
    ) -> Result<Self, RenderError> {
        let parameters = TemplateParameters {
            application: ApplicationTemplateParameter {
                name: app_name,
                base_url,
            },
            services: Some(
                service_configs
                    .iter()
                    .map(|c| ServiceTemplateParameter {
                        name: c.service_name(),
                        container_type: c.container_type().clone(),
                        port: c.port(),
                    })
                    .collect(),
            ),
            service: None,
            user_defined_parameters,
        };

        self.apply_template(&parameters)
    }

    fn apply_template(&self, parameters: &TemplateParameters) -> Result<Self, RenderError> {
        let mut reg = Handlebars::new();
        reg.register_helper("isCompanion", Box::new(is_companion));
        reg.register_helper("isNotCompanion", Box::new(is_not_companion));

        let mut templated_config = self.clone();
        templated_config.set_service_name(&reg.render_template(self.service_name(), &parameters)?);

        if let Some(env) = self.env() {
            templated_config.set_env(Some(env.apply_templating(parameters, &mut reg)?));
        }

        if let Some(files) = self.files() {
            templated_config
                .set_files(Some(apply_templates_with_secrets(&reg, parameters, files)?));
        }

        if let Some(labels) = self.labels() {
            templated_config.set_labels(Some(apply_templates(&reg, parameters, labels)?));
        }

        if let Some(routing) = self.routing() {
            let rule = match &routing.rule {
                Some(rule) => Some(reg.render_template(rule, &parameters)?),
                None => None,
            };

            let additional_middlewares =
                apply_templating_to_middlewares(&reg, parameters, &routing.additional_middlewares)?;

            templated_config.set_routing(Routing {
                rule,
                additional_middlewares,
            });
        }

        Ok(templated_config)
    }
}

impl Environment {
    fn apply_templating(
        &self,
        parameters: &TemplateParameters,
        reg: &mut Handlebars,
    ) -> Result<Self, RenderError> {
        let mut templated_env = Vec::new();

        for e in self.iter() {
            let v = if e.templated() {
                EnvironmentVariable::with_original(
                    SecUtf8::from(reg.render_template(e.value().unsecure(), &parameters)?),
                    e.clone(),
                )
            } else {
                e.clone()
            };
            templated_env.push(v);
        }

        Ok(Environment::new(templated_env))
    }
}

fn is_not_companion<'reg, 'rc>(
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

fn is_companion<'reg, 'rc>(
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

fn apply_templates<K>(
    reg: &Handlebars,
    parameters: &TemplateParameters,
    original_values: &BTreeMap<K, String>,
) -> Result<BTreeMap<K, String>, RenderError>
where
    K: Clone + std::cmp::Ord,
{
    let mut templated_values = BTreeMap::new();

    for (k, v) in original_values {
        templated_values.insert(k.clone(), reg.render_template(v, &parameters)?);
    }

    Ok(templated_values)
}

fn apply_templates_with_secrets<K>(
    reg: &Handlebars,
    parameters: &TemplateParameters,
    original_values: &BTreeMap<K, SecUtf8>,
) -> Result<BTreeMap<K, SecUtf8>, RenderError>
where
    K: Clone + std::cmp::Ord,
{
    let mut templated_values = BTreeMap::new();

    for (k, v) in original_values {
        templated_values.insert(
            k.clone(),
            SecUtf8::from(reg.render_template(v.unsecure(), &parameters)?),
        );
    }

    Ok(templated_values)
}

fn apply_templating_to_middlewares(
    reg: &Handlebars,
    parameters: &TemplateParameters,
    original_values: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, RenderError> {
    let mut templated_values = BTreeMap::new();

    for (k, v) in original_values {
        templated_values.insert(
            k.clone(),
            apply_templating_to_middleware_value(reg, parameters, v)?,
        );
    }

    Ok(templated_values)
}

fn apply_templating_to_middleware_value(
    reg: &Handlebars,
    parameters: &TemplateParameters,
    value: &Value,
) -> Result<Value, RenderError> {
    match value {
        Value::String(v) => Ok(Value::String(reg.render_template(v, &parameters)?)),
        Value::Seq(values) => {
            let mut templated_values = Vec::with_capacity(values.len());
            for v in values.iter() {
                templated_values.push(apply_templating_to_middleware_value(reg, parameters, v)?);
            }
            Ok(Value::Seq(templated_values))
        }
        Value::Map(map) => {
            let mut templated_map = BTreeMap::new();
            for (k, v) in map.iter() {
                templated_map.insert(
                    k.clone(),
                    apply_templating_to_middleware_value(reg, parameters, v)?,
                );
            }
            Ok(Value::Map(templated_map))
        }
        v => Ok(v.clone()),
    }
}

#[derive(Serialize)]
struct TemplateParameters<'a, 'b> {
    application: ApplicationTemplateParameter<'a>,
    services: Option<Vec<ServiceTemplateParameter<'a>>>,
    service: Option<ServiceTemplateParameter<'a>>,
    #[serde(rename = "userDefined")]
    user_defined_parameters: &'b Option<UserDefinedParameters>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplicationTemplateParameter<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: &'a Option<Url>,
}

#[derive(Serialize)]
struct ServiceTemplateParameter<'a> {
    name: &'a str,
    port: u16,
    #[serde(rename = "type")]
    container_type: ContainerType,
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::config::Routing;
    use crate::models::Image;
    use crate::sc;
    use std::path::PathBuf;

    #[test]
    fn should_apply_app_companion_templating_with_service_name() {
        let mut config = ServiceConfig::new(
            String::from("postgres-{{application.name}}"),
            Image::from_str("postgres").unwrap(),
        );
        config.set_env(Some(Environment::new(Vec::new())));

        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &None,
                &Vec::new(),
                &None,
            )
            .unwrap();

        assert_eq!(templated_config.service_name(), "postgres-master");
    }

    #[test]
    fn should_apply_app_companion_templating_with_envs() {
        let mut config = ServiceConfig::new(
            String::from("postgres-db"),
            Image::from_str("postgres").unwrap(),
        );
        config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_templating(
                "DATABASE_SCHEMAS".to_string(),
                SecUtf8::from(
                    r#"{{~#each services~}}
                    {{~name~}},
                {{~/each~}}"#,
                ),
            ),
        ])));

        let service_configs = vec![
            ServiceConfig::new(
                String::from("service-a"),
                Image::from_str("service").unwrap(),
            ),
            ServiceConfig::new(
                String::from("service-b"),
                Image::from_str("service").unwrap(),
            ),
        ];
        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &None,
                &service_configs,
                &None,
            )
            .unwrap();

        let env = templated_config.env().unwrap().get(0).unwrap();
        assert_eq!(env.key(), "DATABASE_SCHEMAS");
        assert_eq!(env.value().unsecure(), "service-a,service-b,");
    }

    #[test]
    fn should_apply_app_companion_templating_with_labels() {
        let mut config = ServiceConfig::new(
            String::from("postgres-db"),
            Image::from_str("postgres").unwrap(),
        );

        let mut labels = BTreeMap::new();
        labels.insert(
            String::from("com.foo.bar"),
            String::from("app-{{application.name}}"),
        );
        config.set_labels(Some(labels));

        let service_configs = vec![
            ServiceConfig::new(
                String::from("service-a"),
                Image::from_str("service").unwrap(),
            ),
            ServiceConfig::new(
                String::from("service-b"),
                Image::from_str("service").unwrap(),
            ),
        ];
        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &None,
                &service_configs,
                &None,
            )
            .unwrap();

        for (k, v) in templated_config.labels().unwrap().iter() {
            assert_eq!(k, "com.foo.bar");
            assert_eq!(v, "app-master");
        }
    }

    #[test]
    fn should_not_apply_app_companion_templating_with_invalid_envs() {
        let mut config = ServiceConfig::new(
            String::from("postgres-db"),
            Image::from_str("postgres").unwrap(),
        );
        config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_templating(
                "DATABASE_SCHEMAS".to_string(),
                SecUtf8::from(
                    r#"{~each services~}}
                    {{~name~}},
                {{~/each~}}"#,
                ),
            ),
        ])));

        let templated_config = config.apply_templating_for_application_companion(
            &AppName::master(),
            &None,
            &[],
            &None,
        );

        assert!(templated_config.is_err());
    }

    #[test]
    fn should_apply_app_companion_templating_with_volumes() {
        let mut config = ServiceConfig::new(
            String::from("nginx-proxy"),
            Image::from_str("nginx").unwrap(),
        );

        let mount_path = PathBuf::from("/etc/ningx/conf.d/default.conf");
        let mut files = BTreeMap::new();
        files.insert(
            mount_path.clone(),
            SecUtf8::from(
                r#"{{#each services}}
location /{{name}} {
    proxy_pass http://{{~name~}};
}
{{/each}}"#,
            ),
        );
        config.set_files(Some(files));

        let service_configs = vec![
            ServiceConfig::new(
                String::from("service-a"),
                Image::from_str("service").unwrap(),
            ),
            ServiceConfig::new(
                String::from("service-b"),
                Image::from_str("service").unwrap(),
            ),
        ];
        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &None,
                &service_configs,
                &None,
            )
            .unwrap();

        assert_eq!(
            templated_config.files().unwrap().get(&mount_path).unwrap(),
            &SecUtf8::from(
                r#"location /service-a {
    proxy_pass http://service-a;
}
location /service-b {
    proxy_pass http://service-b;
}
"#
            )
        );
    }

    #[test]
    fn should_apply_templating_with_is_not_companion_helper() {
        let mut service_a = ServiceConfig::new(
            String::from("service-a"),
            Image::from_str("service").unwrap(),
        );
        service_a.set_container_type(ContainerType::Instance);
        let mut service_b = ServiceConfig::new(
            String::from("service-b"),
            Image::from_str("service").unwrap(),
        );
        service_b.set_container_type(ContainerType::Replica);
        let mut service_c = ServiceConfig::new(
            String::from("service-c"),
            Image::from_str("service").unwrap(),
        );
        service_c.set_container_type(ContainerType::ApplicationCompanion);
        let mut service_d = ServiceConfig::new(
            String::from("service-d"),
            Image::from_str("service").unwrap(),
        );
        service_d.set_container_type(ContainerType::ServiceCompanion);

        let service_configs = vec![service_a, service_b, service_c, service_d];

        let mut config = ServiceConfig::new(
            String::from("nginx-proxy"),
            Image::from_str("nginx").unwrap(),
        );
        let mount_path = PathBuf::from("/etc/ningx/conf.d/default.conf");
        let mut files = BTreeMap::new();
        files.insert(
            mount_path.clone(),
            SecUtf8::from(
                r#"{{#each services}}
{{~#isNotCompanion type}}
location /{{name}} {
    proxy_pass http://{{~name~}};
}
{{/isNotCompanion}}
{{/each}}"#,
            ),
        );
        config.set_files(Some(files));

        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &None,
                &service_configs,
                &None,
            )
            .unwrap();

        assert_eq!(
            templated_config.files().unwrap().get(&mount_path).unwrap(),
            &SecUtf8::from(
                r#"location /service-a {
    proxy_pass http://service-a;
}
location /service-b {
    proxy_pass http://service-b;
}
"#
            )
        );
    }

    #[test]
    fn should_apply_templating_with_is_companion_helper() {
        let mut service_a = ServiceConfig::new(
            String::from("service-a"),
            Image::from_str("service").unwrap(),
        );
        service_a.set_container_type(ContainerType::Instance);
        let mut service_b = ServiceConfig::new(
            String::from("service-b"),
            Image::from_str("service").unwrap(),
        );
        service_b.set_container_type(ContainerType::Replica);
        let mut service_c = ServiceConfig::new(
            String::from("service-c"),
            Image::from_str("service").unwrap(),
        );
        service_c.set_container_type(ContainerType::ApplicationCompanion);
        let mut service_d = ServiceConfig::new(
            String::from("service-d"),
            Image::from_str("service").unwrap(),
        );
        service_d.set_container_type(ContainerType::ServiceCompanion);

        let service_configs = vec![service_a, service_b, service_c, service_d];

        let mut config = ServiceConfig::new(
            String::from("nginx-proxy"),
            Image::from_str("nginx").unwrap(),
        );
        let mount_path = PathBuf::from("/etc/ningx/conf.d/default.conf");
        let mut files = BTreeMap::new();
        files.insert(
            mount_path.clone(),
            SecUtf8::from(
                r#"{{#each services}}
{{~#isCompanion type}}
location /{{name}} {
    proxy_pass {{../application.baseUrl}}{{~name~}};
}
{{/isCompanion}}
{{/each}}"#,
            ),
        );
        config.set_files(Some(files));

        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &Url::from_str("http://my-host/").ok(),
                &service_configs,
                &None,
            )
            .unwrap();

        assert_eq!(
            templated_config
                .files()
                .unwrap()
                .get(&mount_path)
                .unwrap()
                .clone()
                .into_unsecure(),
            String::from(
                r#"location /service-c {
    proxy_pass http://my-host/service-c;
}
location /service-d {
    proxy_pass http://my-host/service-d;
}
"#
            )
        );
    }

    #[test]
    fn should_apply_templating_for_router() {
        let mut config = ServiceConfig::new(
            String::from("api-gateway"),
            Image::from_str("api-gateway").unwrap(),
        );
        config.set_routing(Routing {
            rule: Some("PathPrefix(`/{{application.name}}/`)".to_string()),
            additional_middlewares: BTreeMap::new(),
        });

        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &None,
                &Vec::new(),
                &None,
            )
            .unwrap();

        let routing = templated_config.routing().unwrap();
        assert_eq!(routing.rule, Some(String::from("PathPrefix(`/master/`)")))
    }

    #[test]
    fn should_apply_templating_for_middlewares_with_array_structure() {
        let mut config = ServiceConfig::new(
            String::from("api-gateway"),
            Image::from_str("api-gateway").unwrap(),
        );

        let headers = serde_value::to_value(serde_json::json!({
            "prefixes": [ "/{{application.name}}" ]
        }))
        .unwrap();
        let mut middlewares = BTreeMap::new();
        middlewares.insert("stripPrefixes".to_string(), headers);

        config.set_routing(Routing {
            rule: None,
            additional_middlewares: middlewares,
        });

        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &None,
                &Vec::new(),
                &None,
            )
            .unwrap();

        let routing = templated_config.routing().unwrap();
        assert_eq!(
            routing.additional_middlewares.get("stripPrefixes").unwrap(),
            &serde_value::to_value(serde_json::json!({
                "prefixes": [ "/master" ]
            }))
            .unwrap(),
        );
    }

    #[test]
    fn should_apply_templating_for_middlewares_with_map_structure() {
        let mut config = ServiceConfig::new(
            String::from("api-gateway"),
            Image::from_str("api-gateway").unwrap(),
        );

        let headers = serde_value::to_value(serde_json::json!({
            "customRequestHeaders": {
                "X-Forwarded-Path": "/{{application.name}}"
            }
        }))
        .unwrap();
        let mut middlewares = BTreeMap::new();
        middlewares.insert("headers".to_string(), headers);

        config.set_routing(Routing {
            rule: None,
            additional_middlewares: middlewares,
        });

        let templated_config = config
            .apply_templating_for_application_companion(
                &AppName::master(),
                &None,
                &Vec::new(),
                &None,
            )
            .unwrap();

        let routing = templated_config.routing().unwrap();
        assert_eq!(
            routing.additional_middlewares.get("headers").unwrap(),
            &serde_value::to_value(serde_json::json!({
                "customRequestHeaders": {
                    "X-Forwarded-Path": "/master"
                }
            }))
            .unwrap(),
        )
    }

    #[test]
    fn should_not_apply_templating_for_environment() {
        let mut config = sc!("db", "maria-db");
        config.set_env(Some(Environment::new(vec![EnvironmentVariable::new(
            String::from("DB_USER"),
            SecUtf8::from("admin-{{service.name}}"),
        )])));

        let config = config
            .apply_templating_for_service_companion(
                &AppName::master(),
                &None,
                &sc!("wordpress", "wordpress:alpine"),
                &None,
            )
            .unwrap();

        let env = config.env().unwrap().get(0).unwrap();

        assert_eq!(env.value().unsecure(), "admin-{{service.name}}");
    }

    #[test]
    fn should_apply_templating_for_environment() {
        let mut config = sc!("db", "maria-db");
        config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_templating(
                String::from("DB_USER"),
                SecUtf8::from("admin-{{service.name}}"),
            ),
        ])));

        let config = config
            .apply_templating_for_service_companion(
                &AppName::master(),
                &None,
                &sc!("wordpress", "wordpress:alpine"),
                &None,
            )
            .unwrap();

        let env = config.env().unwrap().get(0).unwrap();

        assert_eq!(env.value().unsecure(), "admin-wordpress");
    }

    #[test]
    fn should_keep_original_environment_variable() {
        let mut config = sc!("db", "maria-db");
        config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_templating(
                String::from("DB_USER"),
                SecUtf8::from("admin-{{service.name}}"),
            ),
        ])));

        let config = config
            .apply_templating_for_service_companion(
                &AppName::master(),
                &None,
                &sc!("wordpress", "wordpress:alpine"),
                &None,
            )
            .unwrap();

        let env = config.env().unwrap().get(0).unwrap();

        assert!(
            env.original().templated(),
            "After applying a template, the environment keep that information"
        );
        assert_eq!(env.original().value().unsecure(), "admin-{{service.name}}");
    }

    #[test]
    fn should_apply_templating_for_environment_with_user_defined_variable() {
        let mut config = sc!("db", "maria-db");
        config.set_env(Some(Environment::new(vec![
            EnvironmentVariable::with_templating(
                String::from("DB_USER"),
                SecUtf8::from("admin-{{userDefined.test}}"),
            ),
        ])));

        let config = config
            .apply_templating_for_service_companion(
                &AppName::master(),
                &None,
                &sc!("wordpress", "wordpress:alpine"),
                &Some(
                    UserDefinedParameters::new(
                        serde_json::json!({
                            "test": "wordpress"
                        }),
                        &jsonschema::validator_for(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "test": {
                                    "type": "string"
                                }
                            }
                        }))
                        .unwrap(),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();

        let env = config.env().unwrap().get(0).unwrap();

        assert_eq!(env.value().unsecure(), "admin-wordpress");
    }
}
