use reqwest::Url;
use std::str::FromStr;

mod common;

#[tokio::test]
async fn should_deploy_nginx() {
    let _ = env_logger::builder().is_test(true).try_init();
    let base_url = Url::from_str("http://localhost:8080").unwrap();

    common::should_deploy_nginx(&base_url, &base_url).await
}

#[tokio::test]
async fn should_replicate_mariadb_with_replicated_env() {
    let _ = env_logger::builder().is_test(true).try_init();
    let base_url = Url::from_str("http://localhost:8080").unwrap();

    common::should_replicate_mariadb_with_replicated_env(&base_url).await
}

#[tokio::test]
async fn should_deploy_nginx_with_bootstrapped_httpd() {
    let _ = env_logger::builder().is_test(true).try_init();
    let base_url = Url::from_str("http://localhost:8080").unwrap();

    common::should_deploy_nginx_with_bootstrapped_httpd(&base_url, &base_url).await
}
