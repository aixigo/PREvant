mod common;
mod container;

use crate::container::{PREvant, Traefik};
use container::{prevant_url, traefik_url};
use log::Level;
use testcontainers::{
    core::logs::consumer::logging_consumer::LoggingConsumer, core::IntoContainerPort,
    runners::AsyncRunner, ImageExt,
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
