use std::time::Duration;

use super::{DeployPayload, Service};
use reqwest::{Client, ClientBuilder, Response, StatusCode, Url};
use uuid::Uuid;

async fn deploy_app(prevant_base_url: &Url, deploy_payload: DeployPayload) -> Result<Uuid, ()> {
    let app_name = Uuid::new_v4();

    log::debug!(
        "Deploying {app_name} with payload: {}",
        serde_json::to_string(&deploy_payload).unwrap()
    );

    for duration in
        exponential_backoff::Backoff::new(10, Duration::from_secs(1), Duration::from_secs(10))
    {
        let res = match Client::new()
            .post(
                prevant_base_url
                    .join(&format!("/api/apps/{app_name}"))
                    .unwrap(),
            )
            .json(&deploy_payload)
            .send()
            .await
        {
            Ok(res) => res,
            Err(err) => match duration {
                Some(duration) => {
                    log::warn!("Cannot deploy app {app_name}, retrying: {err}");
                    tokio::time::sleep(duration).await;
                    continue;
                }
                None => {
                    log::error!("Cannot deploy app {app_name}: {err}");
                    return Err(());
                }
            },
        };

        log::debug!("PREvant responded with {}", res.status());

        match res.status() {
            StatusCode::OK => return Ok(app_name),
            _ => {
                let response_text = res.text().await.unwrap();
                log::error!("Cannot deploy app {app_name}: {response_text}");
                return Err(());
            }
        };
    }
    unreachable!()
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

async fn delete_app(prevant_base_url: &Url, app_name: Uuid) -> Result<(), Response> {
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

async fn make_request(
    traefik_base_url: &Url,
    app_name: &Uuid,
    service_name: Option<&str>,
) -> Response {
    let url = match service_name {
        None => traefik_base_url.join(&format!("/{app_name}/")).unwrap(),
        Some(service_name) => traefik_base_url
            .join(&format!("/{app_name}/{service_name}/"))
            .unwrap(),
    };

    let client = ClientBuilder::new()
        .build()
        .expect("Client should be buildable");

    for duration in
        exponential_backoff::Backoff::new(10, Duration::from_secs(1), Duration::from_secs(10))
    {
        match client.get(url.clone()).send().await {
            Ok(response) => {
                return response;
            }
            Err(err) => match duration {
                Some(duration) => {
                    log::debug!("Could not connect to {url} (retry later): {err}");
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
        DeployPayload::Services(vec![Service::new(
            String::from("nginx"),
            String::from("nginx:alpine"),
        )]),
    )
    .await
    .expect("Should be able to deploy app");

    let mut success = false;

    for duration in
        exponential_backoff::Backoff::new(10, Duration::from_secs(1), Duration::from_secs(10))
    {
        let response = make_request(&traefik_base_url, &app_name, Some("nginx")).await;
        if response.text().await.unwrap().contains("Welcome to nginx!") {
            success = true;
            break;
        }

        if let Some(duration) = duration {
            log::debug!("Did not find nginx welcome message yet");
            tokio::time::sleep(duration).await;
            continue;
        }
    }

    delete_app(&prevant_base_url, app_name)
        .await
        .expect("Should be able to delete app");

    assert!(success, "Cannot make request to nginx");
}

pub async fn should_replicate_mariadb_with_replicated_env(prevant_base_url: &Url) {
    let db_service = Service::new(String::from("db"), String::from("mariadb:lts"))
        .with_replicated_env(
            String::from("MYSQL_RANDOM_ROOT_PASSWORD"),
            String::from("yes"),
        );

    let app_name = deploy_app(&prevant_base_url, DeployPayload::Services(vec![db_service]))
        .await
        .expect("Should be able to deploy app");

    let replicated_app_name = replicate_app(&prevant_base_url, &app_name)
        .await
        .expect("Should be able to replicate app");

    let mut success = false;
    for duration in
        exponential_backoff::Backoff::new(10, Duration::from_secs(1), Duration::from_secs(10))
    {
        let logs = match logs(&prevant_base_url, &replicated_app_name, "db").await {
            Ok(logs) => logs,
            Err(error_response) => {
                let err = error_response.text().await.unwrap();
                log::debug!("Could not connect get logs: {err}");
                String::new()
            }
        };

        if logs.contains("GENERATED ROOT PASSWORD") {
            success = true;
            break;
        }

        if let Some(duration) = duration {
            tokio::time::sleep(duration).await;
            continue;
        }
    }

    delete_app(&prevant_base_url, app_name)
        .await
        .expect("Should be able to delete app");
    delete_app(&prevant_base_url, replicated_app_name)
        .await
        .expect("Should be able to delete app");

    assert!(
        success,
        "Cannot find “GENERATED ROOT PASSWORD” in the container logs after 10 attempts",
    );
}

pub async fn should_deploy_nginx_with_bootstrapped_httpd(
    traefik_base_url: &Url,
    prevant_base_url: &Url,
) {
    let app_name = deploy_app(
        prevant_base_url,
        DeployPayload::UserDefinedAndServices {
            services: vec![Service::new(
                String::from("nginx"),
                String::from("nginx:alpine"),
            )],
            user_defined: serde_json::json!({
                "deployHttpd": "true",
                "abc": 123
            }),
        },
    )
    .await
    .expect("Should be able to deploy app");

    let mut success = false;
    for duration in
        exponential_backoff::Backoff::new(10, Duration::from_secs(1), Duration::from_secs(10))
    {
        let response = make_request(&traefik_base_url, &app_name, None).await;
        if response
            .text()
            .await
            .unwrap()
            .contains("<html><body><h1>It works!</h1></body></html>")
        {
            success = true;
            break;
        }

        if let Some(duration) = duration {
            log::debug!("Did not find Apache httpd welcome message yet");
            tokio::time::sleep(duration).await;
            continue;
        }
    }

    delete_app(&prevant_base_url, app_name)
        .await
        .expect("Should be able to delete app");

    assert!(success, "Cannot make request to Apache httpd");
}

pub async fn should_deploy_nginx_with_cloned_bootstrapped_httpd(
    traefik_base_url: &Url,
    prevant_base_url: &Url,
) {
    let app_name = deploy_app(
        prevant_base_url,
        DeployPayload::UserDefinedAndServices {
            services: vec![Service::new(
                String::from("nginx"),
                String::from("nginx:alpine"),
            )],
            user_defined: serde_json::json!({
                "deployHttpd": "true",
                "abc": 123
            }),
        },
    )
    .await
    .expect("Should be able to deploy app");
    let replicated_app_name = replicate_app(&prevant_base_url, &app_name)
        .await
        .expect("Should be able to replicate app");
    delete_app(&prevant_base_url, app_name)
        .await
        .expect("Should be able to delete app");

    let mut success = false;
    for duration in
        exponential_backoff::Backoff::new(10, Duration::from_secs(1), Duration::from_secs(10))
    {
        let response = make_request(&traefik_base_url, &replicated_app_name, None).await;
        if response
            .text()
            .await
            .unwrap()
            .contains("<html><body><h1>It works!</h1></body></html>")
        {
            success = true;
            break;
        }

        if let Some(duration) = duration {
            log::debug!("Did not find Apache httpd welcome message yet");
            tokio::time::sleep(duration).await;
            continue;
        }
    }

    delete_app(&prevant_base_url, replicated_app_name)
        .await
        .expect("Should be able to delete app");

    assert!(success, "Cannot make request to Apache httpd");
}
