use crate::common::Service;
use reqwest::{Client, Response, StatusCode};
use std::{borrow::Cow, collections::HashMap};
use testcontainers::{
    core::{Mount, WaitFor},
    ContainerAsync, Image,
};
use uuid::Uuid;

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

pub async fn deploy_app(
    prevant: &ContainerAsync<PREvant>,
    services: &Vec<Service>,
) -> Result<Uuid, Response> {
    let port = prevant
        .get_host_port_ipv4(80)
        .await
        .expect("PREvant container must provide a port");

    let app_name = Uuid::new_v4();

    let res = Client::new()
        .post(&format!("http://localhost:{}/api/apps/{}", port, app_name))
        .json(&services)
        .send()
        .await
        .unwrap();

    match res.status() {
        StatusCode::OK => Ok(app_name),
        _ => Err(res),
    }
}

pub async fn replicate_app(
    prevant: &ContainerAsync<PREvant>,
    from_app_name: &Uuid,
) -> Result<Uuid, Response> {
    let port = prevant
        .get_host_port_ipv4(80)
        .await
        .expect("PREvant container must provide a port");

    let app_name = Uuid::new_v4();

    let res = Client::new()
        .post(&format!(
            "http://localhost:{}/api/apps/{}?replicateFrom={}",
            port, app_name, from_app_name
        ))
        .json(&Vec::<Service>::new())
        .send()
        .await
        .unwrap();

    match res.status() {
        StatusCode::OK => Ok(app_name),
        _ => Err(res),
    }
}

pub async fn delete_app(
    prevant: &ContainerAsync<PREvant>,
    app_name: &Uuid,
) -> Result<(), Response> {
    let port = prevant
        .get_host_port_ipv4(80)
        .await
        .expect("PREvant container must provide a port");

    let res = Client::new()
        .delete(&format!("http://localhost:{}/api/apps/{}", port, app_name))
        .send()
        .await
        .unwrap();

    match res.status() {
        StatusCode::OK => Ok(()),
        _ => Err(res),
    }
}

pub async fn logs(
    prevant: &ContainerAsync<PREvant>,
    app_name: &Uuid,
    service_name: &str,
) -> Result<String, Response> {
    let port = prevant
        .get_host_port_ipv4(80)
        .await
        .expect("PREvant container must provide a port");

    let res = Client::new()
        .get(&format!(
            "http://localhost:{}/api/apps/{}/logs/{}",
            port, app_name, service_name
        ))
        .send()
        .await
        .unwrap();

    match res.status() {
        StatusCode::OK => Ok(res.text().await.expect("")),
        _ => Err(res),
    }
}
