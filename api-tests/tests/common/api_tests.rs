use std::time::Duration;

use super::Service;
use reqwest::{Client, ClientBuilder, Response, StatusCode, Url};
use uuid::Uuid;

async fn deploy_app(prevant_base_url: &Url, services: &[Service]) -> Result<Uuid, Response> {
    let app_name = Uuid::new_v4();

    let res = Client::new()
        .post(
            prevant_base_url
                .join(&format!("/api/apps/{app_name}"))
                .unwrap(),
        )
        .json(&services)
        .send()
        .await
        .unwrap();

    match res.status() {
        StatusCode::OK => Ok(app_name),
        _ => Err(res),
    }
}

async fn replicate_app(prevant_base_url: &Url, from_app_name: &Uuid) -> Result<Uuid, Response> {
    let app_name = Uuid::new_v4();

    let res = Client::new()
        .post(
            prevant_base_url
                .join(&format!(
                    "/api/apps/{app_name}?replicateFrom={from_app_name}"
                ))
                .unwrap(),
        )
        .json(&Vec::<Service>::new())
        .send()
        .await
        .unwrap();

    match res.status() {
        StatusCode::OK => Ok(app_name),
        _ => Err(res),
    }
}

async fn delete_app(prevant_base_url: &Url, app_name: &Uuid) -> Result<(), Response> {
    let res = Client::new()
        .delete(
            prevant_base_url
                .join(&format!("/api/apps/{app_name}"))
                .unwrap(),
        )
        .send()
        .await
        .unwrap();

    match res.status() {
        StatusCode::OK => Ok(()),
        _ => Err(res),
    }
}

async fn logs(
    prevant_base_url: &Url,
    app_name: &Uuid,
    service_name: &str,
) -> Result<String, Response> {
    let res = Client::new()
        .get(
            prevant_base_url
                .join(&format!("/api/apps/{app_name}/logs/{service_name}"))
                .unwrap(),
        )
        .send()
        .await
        .unwrap();

    match res.status() {
        StatusCode::OK => Ok(res.text().await.expect("")),
        _ => Err(res),
    }
}
async fn make_request(traefik_base_url: &Url, app_name: &Uuid, service_name: &str) -> Response {
    let attempts = 10;
    let min = Duration::from_secs(1);
    let max = Duration::from_secs(10);

    let client = ClientBuilder::new()
        .build()
        .expect("Client should be buildable");
    for duration in exponential_backoff::Backoff::new(attempts, min, max) {
        match client
            .get(
                traefik_base_url
                    .join(&format!("/{app_name}/{service_name}/"))
                    .unwrap(),
            )
            .send()
            .await
        {
            Ok(response) => {
                return response;
            }
            Err(err) => match duration {
                Some(duration) => {
                    log::debug!(
                        "Could not connect to {app_name}/{service_name} (retry later): {err}"
                    );
                    tokio::time::sleep(duration).await;
                }
                None => panic!("{}", err),
            },
        }
    }

    unreachable!("")
}

pub async fn should_deploy_nginx(traefik_base_url: &Url, prevant_base_url: &Url) {
    let app_name = deploy_app(
        prevant_base_url,
        &vec![Service::new(
            String::from("nginx"),
            String::from("nginx:alpine"),
        )],
    )
    .await
    .expect("Should be able to deploy app");

    let mut i = 0;
    loop {
        let response = make_request(&traefik_base_url, &app_name, "nginx").await;

        if response.text().await.unwrap().contains("Welcome to nginx!") {
            break;
        }
        std::thread::sleep(Duration::from_secs(5));

        i += 1;
        assert!(i < 5, "Cannot make request to nginx after {} attempts", i);
    }

    delete_app(&prevant_base_url, &app_name)
        .await
        .expect("Should be able to delete app");
}

pub async fn should_replicate_mariadb_with_replicated_env(prevant_base_url: &Url) {
    let db_service = Service::new(String::from("db"), String::from("mariadb:10.3.17"))
        .with_replicated_env(
            String::from("MYSQL_RANDOM_ROOT_PASSWORD"),
            String::from("yes"),
        );

    let app_name = deploy_app(&prevant_base_url, &vec![db_service])
        .await
        .expect("Should be able to deploy app");

    let replicated_app_name = replicate_app(&prevant_base_url, &app_name)
        .await
        .expect("Should be able to replicate app");

    let mut i = 0;
    loop {
        if let Ok(logs) = logs(&prevant_base_url, &replicated_app_name, "db").await {
            if logs.contains("GENERATED ROOT PASSWORD") {
                break;
            }
        };

        std::thread::sleep(Duration::from_secs(15));

        i += 1;
        assert!(
            i < 5,
            "Cannot find “GENERATED ROOT PASSWORD” in the container logs after {} attempts",
            i
        );
    }

    delete_app(&prevant_base_url, &app_name)
        .await
        .expect("Should be able to delete app");
    delete_app(&prevant_base_url, &replicated_app_name)
        .await
        .expect("Should be able to delete app");
}
