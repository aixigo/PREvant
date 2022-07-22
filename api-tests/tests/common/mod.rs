use lazy_static::lazy_static;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use testcontainers::clients::Cli;

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

lazy_static! {
    static ref INIT_LOGGER: AtomicBool = AtomicBool::new(true);
}

pub fn docker() -> Cli {
    if let Ok(_) = INIT_LOGGER.compare_exchange(true, false, Ordering::Acquire, Ordering::Relaxed) {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }

    Cli::default()
}
