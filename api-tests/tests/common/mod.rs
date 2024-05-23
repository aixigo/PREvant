use std::collections::HashMap;

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
