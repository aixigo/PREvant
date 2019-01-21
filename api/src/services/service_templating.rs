/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 aixigo AG
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
use crate::models::service::{ContainerType, ServiceConfig};
use handlebars::{Handlebars, TemplateRenderError};
use std::collections::BTreeMap;

pub fn apply_templating_for_application_companion(
    service_config: &ServiceConfig,
    app_name: &String,
    service_configs: &Vec<ServiceConfig>,
) -> Result<ServiceConfig, TemplateRenderError> {
    let parameters = TemplateParameters {
        application: ApplicationTemplateParameter {
            name: app_name.clone(),
        },
        services: Some(
            service_configs
                .iter()
                .map(|c| ServiceTemplateParameter {
                    name: c.get_service_name().clone(),
                    container_type: c.get_container_type().clone(),
                    port: c.get_port(),
                })
                .collect(),
        ),
        service: None,
    };

    let reg = Handlebars::new();

    let mut templated_config = service_config.clone();
    templated_config
        .set_service_name(&reg.render_template(service_config.get_service_name(), &parameters)?);

    if let Some(env) = service_config.get_env() {
        let mut templated_env = Vec::new();

        for e in env {
            templated_env.push(reg.render_template(&e, &parameters)?);
        }

        templated_config.set_env(Some(templated_env));
    }

    if let Some(volumes) = service_config.get_volumes() {
        templated_config.set_volumes(Some(apply_templates(&reg, &parameters, volumes)?));
    }

    if let Some(labels) = service_config.get_labels() {
        templated_config.set_labels(Some(apply_templates(&reg, &parameters, labels)?));
    }

    Ok(templated_config)
}

fn apply_templates(
    reg: &Handlebars,
    parameters: &TemplateParameters,
    original_values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, TemplateRenderError> {
    let mut templated_values = BTreeMap::new();

    for (k, v) in original_values {
        templated_values.insert(k.clone(), reg.render_template(v, &parameters)?);
    }

    Ok(templated_values)
}

#[derive(Serialize)]
struct TemplateParameters {
    application: ApplicationTemplateParameter,
    services: Option<Vec<ServiceTemplateParameter>>,
    service: Option<ServiceTemplateParameter>,
}

#[derive(Serialize)]
struct ApplicationTemplateParameter {
    name: String,
}

#[derive(Serialize)]
struct ServiceTemplateParameter {
    name: String,
    port: u16,
    #[serde(rename = "type")]
    container_type: ContainerType,
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::models::service::Image;
    use std::str::FromStr;

    #[test]
    fn should_apply_app_companion_templating_with_service_name() {
        let env = vec![];
        let mut config = ServiceConfig::new(
            String::from("postgres-{{application.name}}"),
            &Image::from_str("postgres").unwrap(),
        );
        config.set_env(Some(env));

        let service_configs = vec![];
        let templated_config = apply_templating_for_application_companion(
            &config,
            &String::from("master"),
            &service_configs,
        )
        .unwrap();

        assert_eq!(templated_config.get_service_name(), "postgres-master");
    }

    #[test]
    fn should_apply_app_companion_templating_with_envs() {
        let env = vec![String::from(
            r#"DATABASE_SCHEMAS=
                {{~#each services~}}
                    {{~name~}},
                {{~/each~}}"#,
        )];

        let mut config = ServiceConfig::new(
            String::from("postgres-db"),
            &Image::from_str("postgres").unwrap(),
        );
        config.set_env(Some(env));

        let service_configs = vec![
            ServiceConfig::new(
                String::from("service-a"),
                &Image::from_str("service").unwrap(),
            ),
            ServiceConfig::new(
                String::from("service-b"),
                &Image::from_str("service").unwrap(),
            ),
        ];
        let templated_config = apply_templating_for_application_companion(
            &config,
            &String::from("master"),
            &service_configs,
        )
        .unwrap();

        assert_eq!(
            templated_config.get_env().unwrap().get(0).unwrap(),
            "DATABASE_SCHEMAS=service-a,service-b,"
        );
    }

    #[test]
    fn should_apply_app_companion_templating_with_labels() {
        let mut config = ServiceConfig::new(
            String::from("postgres-db"),
            &Image::from_str("postgres").unwrap(),
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
                &Image::from_str("service").unwrap(),
            ),
            ServiceConfig::new(
                String::from("service-b"),
                &Image::from_str("service").unwrap(),
            ),
        ];
        let templated_config = apply_templating_for_application_companion(
            &config,
            &String::from("master"),
            &service_configs,
        )
        .unwrap();

        for (k, v) in templated_config.get_labels().unwrap().iter() {
            assert_eq!(k, "com.foo.bar");
            assert_eq!(v, "app-master");
        }
    }

    #[test]
    fn should_not_apply_app_companion_templating_with_invalid_envs() {
        let env = vec![String::from(
            r#"DATABASE_SCHEMAS=
                {{~each services~}}
                    {{~name~}},
                {{~/each~}}"#,
        )];

        let mut config = ServiceConfig::new(
            String::from("postgres-db"),
            &Image::from_str("postgres").unwrap(),
        );
        config.set_env(Some(env));

        let templated_config =
            apply_templating_for_application_companion(&config, &String::from("master"), &vec![]);

        assert_eq!(templated_config.is_err(), true);
    }

    #[test]
    fn should_apply_app_companion_templating_with_volumes() {
        let mut config = ServiceConfig::new(
            String::from("nginx-proxy"),
            &Image::from_str("nginx").unwrap(),
        );

        let mount_path = String::from("/etc/ningx/conf.d/default.conf");
        let mut volumes = BTreeMap::new();
        volumes.insert(
            mount_path.clone(),
            String::from(
                r#"{{#each services}}
location /{{name}} {
    proxy_pass http://{{~name~}};
}
{{/each}}"#,
            ),
        );
        config.set_volumes(Some(volumes));

        let service_configs = vec![
            ServiceConfig::new(
                String::from("service-a"),
                &Image::from_str("service").unwrap(),
            ),
            ServiceConfig::new(
                String::from("service-b"),
                &Image::from_str("service").unwrap(),
            ),
        ];
        let templated_config = apply_templating_for_application_companion(
            &config,
            &String::from("master"),
            &service_configs,
        )
        .unwrap();

        assert_eq!(
            templated_config
                .get_volumes()
                .unwrap()
                .get(&mount_path)
                .unwrap(),
            r#"
location /service-a {
    proxy_pass http://service-a;
}

location /service-b {
    proxy_pass http://service-b;
}
"#
        );
    }
}
