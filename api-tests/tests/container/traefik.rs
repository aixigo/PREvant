use reqwest::{ClientBuilder, Response};
use std::time::Duration;
use testcontainers::{
    core::{Mount, WaitFor},
    ContainerAsync, Image, ImageArgs,
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

impl ImageArgs for TraefikArgs {
    fn into_iterator(self) -> Box<dyn Iterator<Item = String>> {
        Box::new(
            vec![
                "--api".to_string(),
                "--docker".to_string(),
                "--logLevel=INFO".to_string(),
            ]
            .into_iter(),
        )
    }
}

impl Image for Traefik {
    type Args = TraefikArgs;

    fn name(&self) -> String {
        "traefik".to_string()
    }
    fn tag(&self) -> String {
        "v1.7-alpine".to_string()
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout(
            "Server configuration reloaded on :80",
        )]
    }

    fn mounts(&self) -> Box<dyn Iterator<Item = &Mount> + '_> {
        Box::new(self.mounts.iter())
    }
}

pub async fn make_request(
    traefik: &ContainerAsync<Traefik>,
    app_name: &Uuid,
    service_name: &str,
) -> Response {
    let port = traefik.get_host_port_ipv4(80).await;

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
