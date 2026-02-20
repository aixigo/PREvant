use crate::{
    config::{self, Config},
    http_result::{HttpApiError, HttpResult},
    models::RequestInfo,
};
use handlebars::Handlebars;
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use rocket::State;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(rocket::Responder)]
#[response(status = 200, content_type = "application/yaml")]
pub struct OpenAPI(String);

#[rocket::get("/openapi.yaml")]
pub async fn api_documentation(
    request_info: RequestInfo,
    config: &State<Config>,
) -> HttpResult<OpenAPI> {
    let openapi_path = Path::new("res").join("openapi.yml");

    let mut handlebars = Handlebars::new();
    handlebars
        .register_template_file("openapi", openapi_path)
        .map_err(|e| {
            HttpApiError::from(
                HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                    .detail(e.to_string()),
            )
        })?;

    let mut url = request_info.base_url().clone();
    url.set_path("/api");

    let mut data = BTreeMap::new();
    data.insert("serverUrl", serde_json::Value::String(url.to_string()));
    if matches!(
        config.applications.replication_condition,
        config::ReplicateApplicationCondition::AlwaysFromDefaultApp
    ) {
        data.insert(
            "defaultApplication",
            serde_json::Value::String(config.applications.default_app.to_string()),
        );
    }

    data.insert(
        "examples",
        prepare_for_yaml(match &config.applications.open_api_examples {
            Some(open_api_examples) => read_examples(&open_api_examples).await?,
            None => wordpress_db_example(),
        })?,
    );

    Ok(OpenAPI(handlebars.render("openapi", &data).map_err(
        |e| {
            HttpApiError::from(
                HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                    .detail(e.to_string()),
            )
        },
    )?))
}

fn prepare_for_yaml(examples: serde_json::Value) -> HttpResult<serde_json::Value> {
    match examples {
        Value::Object(map) => Ok(map
            .into_iter()
            .map(|(k, v)| {
                let yaml = serde_norway::to_string(&v)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::Value::String(format!("{line}\n")))
                    .collect::<Vec<_>>();
                (k, yaml)
            })
            .collect::<serde_json::Value>()),
        _ => Err(HttpApiError::from(
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail("The OpenAPI examples must be an object with summary and value as specified by https://swagger.io/docs/specification/v3_0/adding-examples/#examples-for-xml-and-html-data"),
        )),
    }
}

async fn read_examples(examples_path: &PathBuf) -> HttpResult<serde_json::Value> {
    let content = tokio::fs::read_to_string(examples_path)
        .await
        .map_err(|e| {
            HttpApiError::from(
                HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                    .detail(e.to_string()),
            )
        })?;

    serde_norway::from_str::<serde_json::Value>(&content).map_err(|e| {
        HttpApiError::from(
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string()),
        )
    })
}

fn wordpress_db_example() -> serde_json::Value {
    serde_json::json!({
      "wordpress": {
        "summary": "A simple example how one can deploy a Wordpress blog.",
        "value": [
          {
            "serviceName": "db",
            "image": "mariadb",
            "env": {
              "MARIADB_ROOT_PASSWORD": {
                "value": "example",
                "replicate": true
              },
              "MARIADB_USER": {
                "value": "example-user",
                "replicate": true
              },
              "MARIADB_PASSWORD": {
                "value": "my_cool_secret",
                "replicate": true
              },
              "MARIADB_DATABASE": {
                "value": "example-database",
                "replicate": true
              }
            }
          },
          {
            "serviceName": "blog",
            "image": "wordpress",
            "env": {
              "WORDPRESS_DB_HOST": {
                "value": "db",
                "replicate": true
              },
              "WORDPRESS_DB_USER": {
                "value": "example-user",
                "replicate": true
              },
              "WORDPRESS_DB_PASSWORD": {
                "value": "my_cool_secret",
                "replicate": true
              },
              "WORDPRESS_DB_NAME": {
                "value": "example-database",
                "replicate": true
              },
              "WORDPRESS_CONFIG_EXTRA": {
                "value": "define('WP_HOME','http://localhost');\ndefine('WP_SITEURL','http://localhost/{{application.name}}/blog');",
                "replicate": true,
                "templated": true
              }
            }
          }
        ]
      }
    })
}
