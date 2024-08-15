mod common;
mod container;

use crate::common::Service;
use crate::container::{
    delete_app, deploy_app, logs, make_request, replicate_app, PREvant, Traefik,
};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;

#[tokio::test]
async fn should_deploy_nginx() {
    let traefik = Traefik::default()
        .start()
        .await
        .expect("container should be available");
    let prevant = PREvant::default()
        .start()
        .await
        .expect("container should be available");

    let app_name = deploy_app(
        &prevant,
        &vec![Service::new(
            String::from("nginx"),
            String::from("nginx:alpine"),
        )],
    )
    .await
    .expect("Should be able to deploy app");

    let mut i = 0;
    loop {
        let response = make_request(&traefik, &app_name, "nginx").await;

        if response.text().await.unwrap().contains("Welcome to nginx!") {
            break;
        }
        std::thread::sleep(Duration::from_secs(5));

        i += 1;
        assert!(i < 5, "Cannot make request to nginx after {} attempts", i);
    }

    delete_app(&prevant, &app_name)
        .await
        .expect("Should be able to delete app");
}

#[tokio::test]
async fn should_replicate_mariadb_with_replicated_env() {
    let _traefik = Traefik::default()
        .start()
        .await
        .expect("container should be available");
    let prevant = PREvant::default()
        .start()
        .await
        .expect("container should be available");

    let db_service = Service::new(String::from("db"), String::from("mariadb:10.3.17"))
        .with_replicated_env(
            String::from("MYSQL_RANDOM_ROOT_PASSWORD"),
            String::from("yes"),
        );

    let app_name = deploy_app(&prevant, &vec![db_service])
        .await
        .expect("Should be able to deploy app");

    let replicated_app_name = replicate_app(&prevant, &app_name)
        .await
        .expect("Should be able to replicate app");

    let mut i = 0;
    loop {
        if let Ok(logs) = logs(&prevant, &replicated_app_name, "db").await {
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

    delete_app(&prevant, &app_name)
        .await
        .expect("Should be able to delete app");
    delete_app(&prevant, &replicated_app_name)
        .await
        .expect("Should be able to delete app");
}
