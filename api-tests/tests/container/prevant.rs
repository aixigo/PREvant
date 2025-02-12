use reqwest::Url;
use std::{borrow::Cow, collections::HashMap, str::FromStr};
use testcontainers::{
    core::{Mount, WaitFor},
    ContainerAsync, Image,
};

pub struct PREvant {
    env_vars: HashMap<String, String>,
    mounts: Vec<Mount>,
}

impl Default for PREvant {
    fn default() -> Self {
        let mut env_vars = HashMap::new();
        env_vars.insert(String::from("ROCKET_CLI_COLORS"), String::from("false"));

        let mut mounts = Vec::new();
        mounts.push(Mount::bind_mount(
            "/var/run/docker.sock",
            "/var/run/docker.sock",
        ));

        Self { mounts, env_vars }
    }
}

impl Image for PREvant {
    fn name(&self) -> &str {
        "aixigo/prevant"
    }
    fn tag(&self) -> &str {
        "latest"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stderr("Rocket has launched from")]
    }

    fn mounts(&self) -> impl IntoIterator<Item = &Mount> {
        Box::new(self.mounts.iter())
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        Box::new(self.env_vars.iter())
    }
}

pub async fn prevant_url(prevant: &ContainerAsync<PREvant>) -> Url {
    let port = prevant
        .get_host_port_ipv4(80)
        .await
        .expect("PREvant container must provide a port");

    Url::from_str(&format!("http://localhost:{port}")).unwrap()
}
