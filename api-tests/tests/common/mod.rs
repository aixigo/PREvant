pub use api_tests::{
    should_deploy_nginx, should_deploy_nginx_with_bootstrapped_httpd,
    should_replicate_mariadb_with_replicated_env,
};
use std::collections::HashMap;

mod api_tests;

#[derive(serde::Serialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum DeployPayload {
    Services(Vec<Service>),
    UserDefinedAndServices {
        #[serde(rename = "userDefined")]
        user_defined: serde_json::Value,
        services: Vec<Service>,
    },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    service_name: String,
    image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    env: Option<HashMap<String, Env>>,
}

#[derive(serde::Serialize)]
struct Env {
    value: String,
    replicate: bool,
}

impl Service {
    pub fn new(name: String, image: String) -> Self {
        Service {
            service_name: name,
            image,
            env: None,
        }
    }

    pub fn with_replicated_env(mut self, env_name: String, env_value: String) -> Self {
        let mut e = match self.env.take() {
            Some(e) => e,
            None => HashMap::new(),
        };

        e.insert(
            env_name,
            Env {
                value: env_value,
                replicate: true,
            },
        );

        self.env = Some(e);
        self
    }
}
