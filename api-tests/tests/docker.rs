mod common;
mod container;

use crate::container::{PREvant, Traefik};
use container::{prevant_url, traefik_url};
use log::Level;
use testcontainers::{
    core::logs::consumer::logging_consumer::LoggingConsumer, runners::AsyncRunner, ImageExt,
};

#[tokio::test]
async fn should_deploy_nginx() {
    env_logger::init();
    let traefik = Traefik::default()
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

#[tokio::test]
async fn should_replicate_mariadb_with_replicated_env() {
    env_logger::init();
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
