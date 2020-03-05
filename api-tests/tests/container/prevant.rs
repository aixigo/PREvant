use crate::common::Service;
use reqwest::{Client, Response, StatusCode};
use std::collections::HashMap;
use testcontainers::{Container, Docker, Image, WaitForMessage};
use uuid::Uuid;

#[derive(Default)]
pub struct PREvant;

impl Image for PREvant {
    type Args = Vec<String>;
    type EnvVars = HashMap<String, String>;
    type Volumes = HashMap<String, String>;

    fn descriptor(&self) -> String {
        "aixigo/prevant".to_string()
    }

    fn wait_until_ready<D: Docker>(&self, container: &Container<'_, D, Self>) {
        container
            .logs()
            .stderr
            .wait_for_message("launched")
            .unwrap();
    }

    fn args(&self) -> Self::Args {
        Vec::with_capacity(0)
    }

    fn env_vars(&self) -> Self::EnvVars {
        HashMap::new()
    }

    fn volumes(&self) -> <Self as Image>::Volumes {
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

pub async fn deploy_app<D>(
    prevant: &Container<'_, D, PREvant>,
    services: &Vec<Service>,
) -> Result<Uuid, Response>
where
    D: Docker,
{
    let port = prevant
        .get_host_port(80)
        .expect("PREvant should expose port 80");

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

pub async fn replicate_app<D>(
    prevant: &Container<'_, D, PREvant>,
    from_app_name: &Uuid,
) -> Result<Uuid, Response>
where
    D: Docker,
{
    let port = prevant
        .get_host_port(80)
        .expect("PREvant should expose port 80");

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

pub async fn delete_app<D>(
    prevant: &Container<'_, D, PREvant>,
    app_name: &Uuid,
) -> Result<(), Response>
where
    D: Docker,
{
    let port = prevant
        .get_host_port(80)
        .expect("PREvant should expose port 80");

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

pub async fn logs<D>(
    prevant: &Container<'_, D, PREvant>,
    app_name: &Uuid,
    service_name: &str,
) -> Result<String, Response>
where
    D: Docker,
{
    let port = prevant
        .get_host_port(80)
        .expect("PREvant should expose port 80");

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
