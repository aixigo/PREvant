use reqwest::{ClientBuilder, Response};
use std::collections::HashMap;
use std::time::Duration;
use testcontainers::{Container, Docker, Image, WaitForMessage};
use uuid::Uuid;

#[derive(Default)]
pub struct Traefik;

impl Image for Traefik {
    type Args = Vec<String>;
    type EnvVars = HashMap<String, String>;
    type Volumes = HashMap<String, String>;

    fn descriptor(&self) -> String {
        "traefik:v1.7-alpine".to_string()
    }

    fn wait_until_ready<D: Docker>(&self, container: &Container<D, Self>) {
        container
            .logs()
            .stdout
            .wait_for_message("Server configuration reloaded on :80")
            .unwrap();
    }

    fn args(&self) -> Self::Args {
        vec![
            String::from("--api"),
            String::from("--docker"),
            String::from("--logLevel=INFO"),
        ]
    }

    fn env_vars(&self) -> Self::EnvVars {
        HashMap::new()
    }

    fn volumes(&self) -> Self::Volumes {
        let mut volumes = HashMap::new();
        volumes.insert(
            String::from("/var/run/docker.sock"),
            String::from("/var/run/docker.sock"),
        );
        volumes
    }

    fn with_args(self, _arguments: Self::Args) -> Self {
        self
    }
}

pub async fn make_request<D>(
    traefik: &Container<'_, D, Traefik>,
    app_name: &Uuid,
    service_name: &str,
) -> Response
where
    D: Docker,
{
    let port = traefik
        .get_host_port(80)
        .expect("Traefik should expose port 80");

    let client = ClientBuilder::new()
        .connect_timeout(Duration::from_millis(2_000))
        .build()
        .expect("reqwest client should be buildable");

    client
        .get(&format!(
            "http://localhost:{}/{}/{}/",
            port, app_name, service_name
        ))
        .send()
        .await
        .expect("a response")
}
