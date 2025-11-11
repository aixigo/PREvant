mod common;
mod container;

use crate::container::{PREvant, Traefik};
use container::{prevant_url, traefik_url};
use log::Level;
use reqwest::Url;
use std::{path::Path, str::FromStr};
use testcontainers::{
    compose::DockerCompose,
    core::{logs::consumer::logging_consumer::LoggingConsumer, IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    ImageExt,
};

#[tokio::test]
async fn should_deploy_nginx() {
    let _ = env_logger::builder().is_test(true).try_init();
    let traefik = Traefik::default()
        // TODO: somehow the mapping of ports is not deterministic: When this line is missing, the
        // container gets a random exposed port but querying the port is often but not always off
        // by one or two.
        .with_mapped_port(8080, 80.tcp())
        .with_log_consumer(
            LoggingConsumer::new()
                .with_stdout_level(Level::Debug)
                .with_stderr_level(Level::Debug),
        )
        .start()
        .await
        .expect("container should be available");
    let prevant = PREvant::default()
        .with_log_consumer(
            LoggingConsumer::new()
                .with_stdout_level(Level::Debug)
                .with_stderr_level(Level::Debug),
        )
        .start()
        .await
        .expect("container should be available");

    common::should_deploy_nginx(&traefik_url(&traefik).await, &prevant_url(&prevant).await).await
}

// TODO: make sure that the compose tests run in CI
#[cfg_attr(feature = "ci", ignore)]
#[tokio::test]
async fn should_deploy_nginx_in_docker_compose_with_postgres() {
    let _ = env_logger::builder().is_test(true).try_init();

    let docker_compose_file = std::fs::canonicalize(
        &std::path::absolute(Path::new("../examples/Docker/docker-compose.yml")).unwrap(),
    )
    .unwrap();

    let mut compose = DockerCompose::with_local_client(&[docker_compose_file])
        .with_env("POSTGRES_PASSWORD", "example.1234")
        .with_wait_for_service(
            "prevant",
            WaitFor::message_on_stderr("Rocket has launched from"),
        )
        .with_wait_for_service(
            "traefik",
            WaitFor::message_on_stdout("Server configuration reloaded on :80"),
        );

    compose.up().await.unwrap();

    let traefik_url = Url::from_str("http://localhost").unwrap();
    let prevant_url = Url::from_str("http://localhost").unwrap();

    std::thread::sleep(std::time::Duration::from_secs(2));

    common::should_deploy_nginx(&traefik_url, &prevant_url).await
}

#[tokio::test]
async fn should_replicate_mariadb_with_replicated_env() {
    let _ = env_logger::builder().is_test(true).try_init();
    let _traefik = Traefik::default()
        .with_log_consumer(
            LoggingConsumer::new()
                .with_stdout_level(Level::Debug)
                .with_stderr_level(Level::Debug),
        )
        .start()
        .await
        .expect("container should be available");
    let prevant = PREvant::default()
        .with_log_consumer(
            LoggingConsumer::new()
                .with_stdout_level(Level::Debug)
                .with_stderr_level(Level::Debug),
        )
        .start()
        .await
        .expect("container should be available");

    common::should_replicate_mariadb_with_replicated_env(&prevant_url(&prevant).await).await
}
