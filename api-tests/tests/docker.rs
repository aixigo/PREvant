mod common;
mod container;

use crate::container::{PREvant, Traefik};
use container::{prevant_url, traefik_url};
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

    common::should_deploy_nginx(&traefik_url(&traefik).await, &prevant_url(&prevant).await).await
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

    common::should_replicate_mariadb_with_replicated_env(&prevant_url(&prevant).await).await
}
