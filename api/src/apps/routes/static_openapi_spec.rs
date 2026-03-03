use crate::{
    apps::{routes::AppNameFromParam, Apps, HostMetaCache},
    config::Config,
    http_result::HttpResult,
    models::RequestInfo,
};
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use rocket::State;
use serde_norway::Value;

#[rocket::get("/<app_name>/static-open-api-spec/<service_name>", rank = 1)]
pub(super) async fn static_open_api_spec(
    apps: &State<Apps>,
    config: &State<Config>,
    app_name: Result<AppNameFromParam, HttpApiProblem>,
    service_name: &str,
    request_info: RequestInfo,
    host_meta_cache: HostMetaCache,
) -> HttpResult<String> {
    let app_name = app_name?.0;

    let Some(service) = apps.fetch_service_of_app(&app_name, service_name).await? else {
        return Err(HttpApiProblem::with_title_and_type(StatusCode::NOT_FOUND).into());
    };

    let Some(static_host_config) =
        config
            .static_host_meta(&service.blueprint_config.image)
            .map_err(|e| {
                HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                    .detail(e.to_string())
            })?
    else {
        return Err(HttpApiProblem::with_title_and_type(StatusCode::NOT_FOUND).into());
    };

    let Some(open_api_spec) = static_host_config.open_api_spec.as_ref() else {
        return Err(HttpApiProblem::with_title_and_type(StatusCode::NOT_FOUND).into());
    };

    let service =
        host_meta_cache.assign_host_meta_data_to_service(&app_name, service, &request_info);
    let Some(mut public_service_url) = service.service_url else {
        return Err(
            HttpApiProblem::with_title_and_type(StatusCode::PRECONDITION_REQUIRED)
                .detail("The service has no public UR.")
                .into(),
        );
    };

    public_service_url = if let Some(path) = open_api_spec.sub_path {
        public_service_url.join(path).map_err(|e| {
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string())
        })?
    } else {
        public_service_url
    };

    let body = reqwest::get(open_api_spec.source_url.to_string())
        .await
        .map_err(|e| {
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string())
        })?
        .text()
        .await
        .map_err(|e| {
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string())
        })?;

    let mut v: Value = serde_norway::from_str(&body).map_err(|e| {
        HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR).detail(e.to_string())
    })?;

    v["servers"] = serde_norway::from_str(&format!(
        r#"
        - url: {public_service_url}
    "#
    ))
    .unwrap();

    Ok(serde_norway::to_string(&v).unwrap())
}
