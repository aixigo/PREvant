mod common;
mod container;

use crate::common::{docker, Service};
use crate::container::{
    delete_app, deploy_app, logs, make_request, replicate_app, PREvant, Traefik,
};
use std::time::Duration;

#[tokio::test]
async fn should_deploy_nginx() {
    let docker = docker();
    let traefik = docker.run(Traefik::default());
    let prevant = docker.run(PREvant::default());

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
    let docker = docker();
    let _traefik = docker.run(Traefik::default());
    let prevant = docker.run(PREvant::default());

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
