use reqwest::Url;
use std::str::FromStr;

mod common;

#[tokio::test]
async fn should_deploy_nginx() {
    let base_url = Url::from_str("http://localhost").unwrap();

    common::should_deploy_nginx(&base_url, &base_url).await
}

#[tokio::test]
async fn should_replicate_mariadb_with_replicated_env() {
    let base_url = Url::from_str("http://localhost").unwrap();

    common::should_replicate_mariadb_with_replicated_env(&base_url).await
}
