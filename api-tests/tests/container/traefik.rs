use reqwest::{ClientBuilder, Response};
use std::time::Duration;
use testcontainers::{
    core::{Mount, WaitFor},
    ContainerAsync, Image,
};
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct TraefikArgs;

pub struct Traefik {
    mounts: Vec<Mount>,
}

impl Default for Traefik {
    fn default() -> Self {
        let mut mounts = Vec::new();
        mounts.push(Mount::bind_mount(
            "/var/run/docker.sock",
            "/var/run/docker.sock",
        ));
        Self { mounts }
    }
}

impl Image for Traefik {
    fn cmd(&self) -> impl IntoIterator<Item = impl Into<std::borrow::Cow<'_, str>>> {
        Box::new(
            vec![
                "--api".to_string(),
                "--docker".to_string(),
                "--logLevel=INFO".to_string(),
            ]
            .into_iter(),
        )
    }

    fn name(&self) -> &str {
        "traefik"
    }
    fn tag(&self) -> &str {
        "v1.7-alpine"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout(
            "Server configuration reloaded on :80",
        )]
    }

    fn mounts(&self) -> impl IntoIterator<Item = &Mount> {
        Box::new(self.mounts.iter())
    }
}

pub async fn make_request(
    traefik: &ContainerAsync<Traefik>,
    app_name: &Uuid,
    service_name: &str,
) -> Response {
    let port = traefik
        .get_host_port_ipv4(80)
        .await
        .expect("Traefik must export port");

    backoff::future::retry(
        backoff::ExponentialBackoffBuilder::new()
            .with_max_elapsed_time(Some(std::time::Duration::from_secs(60)))
            .build(),
        || async {
            let client = ClientBuilder::new()
                .connect_timeout(Duration::from_millis(2_000))
                .build()?;

            Ok(client
                .get(&format!(
                    "http://localhost:{port}/{app_name}/{service_name}/",
                ))
                .send()
                .await?)
        },
    )
    .await
    .expect("a response")
}
