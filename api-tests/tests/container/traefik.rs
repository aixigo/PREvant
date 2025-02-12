use std::str::FromStr;

use reqwest::Url;
use testcontainers::{
    core::{Mount, WaitFor},
    ContainerAsync, Image,
};

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
    fn expose_ports(&self) -> &[testcontainers::core::ContainerPort] {
        &[testcontainers::core::ContainerPort::Tcp(80)]
    }

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

pub async fn traefik_url(traefik: &ContainerAsync<Traefik>) -> Url {
    let port = traefik
        .get_host_port_ipv6(80)
        .await
        .expect("Traefik must export port");

    Url::from_str(&format!("http://localhost:{port}")).unwrap()
}
