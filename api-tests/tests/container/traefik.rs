use std::str::FromStr;

use reqwest::Url;
use testcontainers::{
    core::{Mount, WaitFor},
    ContainerAsync, Image,
};

pub enum TraefikVersion {
    // TODO: decide if we want to skip the usage of V1 in GitHub actions. The docker daemon dropped
    // the supported Docker version of Traefik. Maybe, podman is still available to test that.
    V1,
    V2,
    V3,
}

pub struct Traefik {
    mounts: Vec<Mount>,
    version: TraefikVersion,
}

impl Traefik {
    pub fn with_major_version(version: TraefikVersion) -> Self {
        let mut mounts = Vec::new();
        mounts.push(Mount::bind_mount(
            "/var/run/docker.sock",
            "/var/run/docker.sock",
        ));
        Self { mounts, version }
    }
}

impl Image for Traefik {
    fn expose_ports(&self) -> &[testcontainers::core::ContainerPort] {
        &[testcontainers::core::ContainerPort::Tcp(80)]
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<std::borrow::Cow<'_, str>>> {
        let docker_provider_argument = match self.version {
            TraefikVersion::V1 => String::from("--docker"),
            TraefikVersion::V2 | TraefikVersion::V3 => String::from("--providers.docker"),
        };
        let log_level_argument = match self.version {
            TraefikVersion::V1 => String::from("--logLevel=INFO"),
            TraefikVersion::V2 | TraefikVersion::V3 => String::from("--log.level=INFO"),
        };
        Box::new(
            vec![
                "--api".to_string(),
                docker_provider_argument,
                log_level_argument,
                // TODO: remove
                "--api.insecure".to_string(),
            ]
            .into_iter(),
        )
    }

    fn name(&self) -> &str {
        "traefik"
    }
    fn tag(&self) -> &str {
        match self.version {
            TraefikVersion::V1 => "v1.7-alpine",
            TraefikVersion::V2 => "v2",
            TraefikVersion::V3 => "v3",
        }
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        match self.version {
            TraefikVersion::V1 => vec![WaitFor::message_on_stdout(
                "Server configuration reloaded on :80",
            )],
            TraefikVersion::V2 => vec![WaitFor::message_on_stdout(
                "Configuration loaded from flags.",
            )],
            TraefikVersion::V3 => vec![WaitFor::message_on_stdout(
                "Starting provider *docker.Provider",
            )],
        }
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
