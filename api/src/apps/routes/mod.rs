/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */

use crate::apps::HostMetaCache;
use crate::apps::{Apps, AppsError};
use crate::http_result::{HttpApiError, HttpResult};
use crate::models::request_info::RequestInfo;
use crate::models::service::{Service, ServiceStatus};
use crate::models::ServiceConfig;
use crate::models::{AppName, AppNameError};
use crate::models::{AppStatusChangeId, AppStatusChangeIdError};
use http_api_problem::{HttpApiProblem, StatusCode};
use multimap::MultiMap;
use regex::Regex;
use rocket::http::Status;
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::{Responder, Response};
use rocket::serde::json::Json;
use rocket::State;
use std::future::Future;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::time::timeout;

mod logs;

pub fn apps_routes() -> Vec<rocket::Route> {
    rocket::routes![
        apps,
        delete_app,
        create_app,
        logs::logs,
        logs::stream_logs,
        change_status,
        status_change,
    ]
}

#[get("/", format = "application/json")]
async fn apps(
    apps: &State<Arc<Apps>>,
    request_info: RequestInfo,
    host_meta_cache: &State<HostMetaCache>,
) -> HttpResult<Json<MultiMap<AppName, Service>>> {
    let services = apps.get_apps().await?;
    Ok(Json(
        host_meta_cache.update_meta_data(services, &request_info),
    ))
}

#[get("/<app_name>/status-changes/<status_id>", format = "application/json")]
async fn status_change(
    app_name: Result<AppName, AppNameError>,
    status_id: Result<AppStatusChangeId, AppStatusChangeIdError>,
    apps: &State<Arc<Apps>>,
    options: RunOptions,
) -> HttpResult<AsyncCompletion<Json<Vec<Service>>>> {
    let app_name = app_name?;
    let status_id = status_id?;

    let apps = (**apps).clone();
    let future = async move { apps.wait_for_status_change(&status_id).await };

    match spawn_with_options(options, future).await? {
        Poll::Pending => Ok(AsyncCompletion::Pending(app_name, status_id)),
        Poll::Ready(Ok(_)) => Err(HttpApiProblem::with_title(StatusCode::NOT_FOUND).into()),
        Poll::Ready(Err(err)) => Err(err.into()),
    }
}

#[delete("/<app_name>")]
pub async fn delete_app(
    app_name: Result<AppName, AppNameError>,
    apps: &State<Arc<Apps>>,
    options: RunOptions,
) -> HttpResult<AsyncCompletion<Json<Vec<Service>>>> {
    let app_name = app_name?;
    let app_name_cloned = app_name.clone();
    let status_id = AppStatusChangeId::new();

    let apps = (**apps).clone();
    let future = async move { apps.delete_app(&app_name, &status_id).await };

    match spawn_with_options(options, future).await? {
        Poll::Pending => Ok(AsyncCompletion::Pending(app_name_cloned, status_id)),
        Poll::Ready(Ok(services)) => Ok(AsyncCompletion::Ready(Json(services))),
        Poll::Ready(Err(err)) => Err(err.into()),
    }
}

pub async fn delete_app_sync(
    app_name: Result<AppName, AppNameError>,
    apps: &State<Arc<Apps>>,
) -> HttpResult<Json<Vec<Service>>> {
    match delete_app(app_name, apps, RunOptions::Sync).await? {
        AsyncCompletion::Pending(_, _) => {
            Err(HttpApiProblem::with_title(StatusCode::INTERNAL_SERVER_ERROR).into())
        }
        AsyncCompletion::Ready(result) => Ok(result),
    }
}

#[post(
    "/<app_name>?<create_app_form..>",
    format = "application/json",
    data = "<service_configs>"
)]
pub async fn create_app(
    app_name: Result<AppName, AppNameError>,
    apps: &State<Arc<Apps>>,
    create_app_form: CreateAppOptions,
    service_configs: Result<Json<Vec<ServiceConfig>>, rocket::serde::json::Error<'_>>,
    options: RunOptions,
) -> HttpResult<AsyncCompletion<Json<Vec<Service>>>> {
    let service_configs = service_configs.map_err(|e| {
        let detail = match e {
            rocket::serde::json::Error::Parse(_, e) => e.to_string(),
            e => e.to_string(),
        };

        HttpApiProblem::with_title_and_type(StatusCode::BAD_REQUEST).detail(detail)
    })?;

    let status_id = AppStatusChangeId::new();
    let app_name = app_name?;
    let app_name_cloned = app_name.clone();
    let replicate_from = create_app_form.replicate_from().clone();

    let apps = (**apps).clone();
    let future = async move {
        apps.create_or_update(
            &app_name.clone(),
            &status_id,
            replicate_from,
            &service_configs,
        )
        .await
    };

    match spawn_with_options(options, future).await? {
        Poll::Pending => Ok(AsyncCompletion::Pending(app_name_cloned, status_id)),
        Poll::Ready(Ok(services)) => Ok(AsyncCompletion::Ready(Json(services))),
        Poll::Ready(Err(err)) => Err(err.into()),
    }
}

#[put(
    "/<app_name>/states/<service_name>",
    format = "application/json",
    data = "<status_data>"
)]
async fn change_status(
    app_name: Result<AppName, AppNameError>,
    service_name: String,
    apps: &State<Arc<Apps>>,
    status_data: Json<ServiceStatusData>,
) -> HttpResult<ServiceStatusResponse> {
    let app_name = app_name?;
    let status = status_data.status.clone();

    let service = apps.change_status(&app_name, &service_name, status).await?;

    Ok(ServiceStatusResponse { service })
}

#[derive(Debug, PartialEq)]
pub enum RunOptions {
    Sync,
    Async { wait: Option<Duration> },
}

pub async fn spawn_with_options<F>(options: RunOptions, future: F) -> HttpResult<Poll<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let join_handle = tokio::spawn(future);

    match options {
        RunOptions::Sync => Ok(Poll::Ready(join_handle.await.map_err(map_join_error)?)),
        RunOptions::Async { wait: None } => Ok(Poll::Pending),
        RunOptions::Async {
            wait: Some(duration),
        } => {
            match tokio::spawn(timeout(duration, join_handle))
                .await
                .map_err(map_join_error)?
            {
                // Execution completed before timeout
                Ok(Ok(result)) => Ok(Poll::Ready(result)),
                // JoinError occurred before timeout
                Ok(Err(err)) => Err(map_join_error(err)),
                // Timeout elapsed while waiting for future
                Err(_) => Ok(Poll::Pending),
            }
        }
    }
}

fn map_join_error(err: tokio::task::JoinError) -> HttpApiError {
    HttpApiProblem::with_title(StatusCode::INTERNAL_SERVER_ERROR)
        .detail(format!("{}", err))
        .into()
}

#[derive(FromForm)]
pub struct CreateAppOptions {
    #[field(name = "replicateFrom")]
    replicate_from: Option<AppName>,
}

impl CreateAppOptions {
    fn replicate_from(&self) -> &Option<AppName> {
        &self.replicate_from
    }
}

#[derive(Serialize, Deserialize)]
pub struct ServiceStatusData {
    status: ServiceStatus,
}

pub struct ServiceStatusResponse {
    service: Option<Service>,
}

pub enum AsyncCompletion<T> {
    Pending(AppName, AppStatusChangeId),
    Ready(T),
}

impl<'r, T> Responder<'r, 'static> for AsyncCompletion<T>
where
    T: Responder<'r, 'static>,
{
    fn respond_to(self, request: &'r Request) -> Result<Response<'static>, Status> {
        match self {
            AsyncCompletion::Pending(app_name, status_id) => {
                let url = format!("/api/apps/{}/status-changes/{}", app_name, status_id);
                Response::build()
                    .status(Status::Accepted)
                    .raw_header("Location", url)
                    .ok()
            }
            AsyncCompletion::Ready(result) => result.respond_to(request),
        }
    }
}

impl<'r> Responder<'r, 'static> for ServiceStatusResponse {
    fn respond_to(self, _request: &'r Request) -> Result<Response<'static>, Status> {
        match self.service {
            Some(_service) => Response::build().status(Status::Accepted).ok(),
            None => Response::build().status(Status::NotFound).ok(),
        }
    }
}

impl From<AppsError> for HttpApiError {
    fn from(error: AppsError) -> Self {
        let status = match &error {
            AppsError::UnableToResolveImage { error } => match error {
                crate::registry::RegistryError::ImageNotFound { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            AppsError::AppNotFound { .. } => StatusCode::NOT_FOUND,
            AppsError::AppIsInDeployment { .. } => StatusCode::CONFLICT,
            AppsError::AppIsInDeletion { .. } => StatusCode::CONFLICT,
            AppsError::FailedToParseTraefikRule { .. }
            | AppsError::InfrastructureError { .. }
            | AppsError::InvalidServerConfiguration { .. }
            | AppsError::InvalidTemplateFormat { .. }
            | AppsError::InvalidDeploymentHook => {
                error!("Internal server error: {}", error);
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };

        HttpApiProblem::with_title_and_type(status)
            .detail(format!("{}", error))
            .into()
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for RunOptions {
    type Error = &'static str;

    async fn from_request(request: &'r Request<'_>) -> rocket::request::Outcome<Self, Self::Error> {
        let headers = request.headers().get("Prefer").collect::<Vec<_>>();

        let mut run_options = RunOptions::Sync;
        let mut wait = None;
        lazy_static! {
            static ref RE: Regex = Regex::new(r"^wait=(\d+)$").unwrap();
        }

        for header in headers
            .iter()
            .flat_map(|header| header.split(','))
            .map(str::trim)
        {
            if header == "respond-async" {
                run_options = RunOptions::Async { wait: None };
                continue;
            }

            if let Some(wait_capture) = RE.captures(header) {
                wait = Some(Duration::from_secs(
                    wait_capture
                        .get(1)
                        .unwrap()
                        .as_str()
                        .parse::<u64>()
                        .unwrap(),
                ));
            }
        }

        Outcome::Success(match (run_options, wait) {
            (RunOptions::Sync, _) => RunOptions::Sync,
            (RunOptions::Async { .. }, wait) => RunOptions::Async { wait },
        })
    }
}

#[cfg(test)]
mod tests {
    mod parse_run_options_from_request {
        use crate::apps::routes::*;
        use rocket::http::Header;
        use rocket::local::asynchronous::Client;

        #[tokio::test]
        async fn without_prefer_header() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client.get("/");
            let request = get.inner();

            let run_options = RunOptions::from_request(request).await.succeeded();

            assert_eq!(run_options, Some(RunOptions::Sync));
        }

        #[tokio::test]
        async fn with_unknown_prefer_header_content() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get("/")
                .header(Header::new("Prefer", "handling=lenient"));
            let request = get.inner();

            let run_options = RunOptions::from_request(request).await.succeeded();

            assert_eq!(run_options, Some(RunOptions::Sync));
        }

        #[tokio::test]
        async fn prefer_async_without_wait() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get("/")
                .header(Header::new("Prefer", "respond-async"));
            let request = get.inner();

            let run_options = RunOptions::from_request(request).await.succeeded();

            assert_eq!(run_options, Some(RunOptions::Async { wait: None }));
        }

        #[tokio::test]
        async fn prefer_async_with_wait() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get("/")
                .header(Header::new("Prefer", "respond-async, wait=100"));
            let request = get.inner();

            let run_options = RunOptions::from_request(request).await.succeeded();

            assert_eq!(
                run_options,
                Some(RunOptions::Async {
                    wait: Some(Duration::from_secs(100))
                })
            );
        }

        #[tokio::test]
        async fn prefer_async_and_with_second_wait() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get("/")
                .header(Header::new("Prefer", "respond-async"))
                .header(Header::new("Prefer", "wait=100"));
            let request = get.inner();

            let run_options = RunOptions::from_request(request).await.succeeded();

            assert_eq!(
                run_options,
                Some(RunOptions::Async {
                    wait: Some(Duration::from_secs(100))
                })
            );
        }

        #[tokio::test]
        async fn with_malformed_prefer_header() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client.get("/").header(Header::new("Prefer", "abcd"));
            let request = get.inner();

            let run_options = RunOptions::from_request(request).await.succeeded();

            assert_eq!(run_options, Some(RunOptions::Sync));
        }
    }

    mod url_rendering {
        use crate::apps::{AppsService, HostMetaCache};
        use crate::infrastructure::Dummy;
        use crate::models::{AppName, AppStatusChangeId};
        use crate::sc;
        use assert_json_diff::assert_json_include;
        use rocket::http::ContentType;
        use rocket::http::Header;
        use rocket::http::Status;
        use rocket::local::asynchronous::Client;
        use serde_json::json;
        use serde_json::Value;
        use std::convert::From;
        use std::sync::Arc;

        async fn set_up_rocket_with_dummy_infrastructure_and_a_running_app(
            host_meta_cache: HostMetaCache,
        ) -> Result<Client, crate::apps::AppsServiceError> {
            let infrastructure = Box::new(Dummy::new());
            let apps = Arc::new(AppsService::new(Default::default(), infrastructure).unwrap());
            let _result = apps
                .create_or_update(
                    &AppName::master(),
                    &AppStatusChangeId::new(),
                    None,
                    &vec![sc!("service-a")],
                )
                .await?;

            let rocket = rocket::build()
                .manage(host_meta_cache)
                .manage(apps)
                .mount("/", routes![crate::apps::routes::apps])
                .mount("/api/apps", crate::apps::apps_routes());
            Ok(Client::tracked(rocket).await.expect("valid rocket"))
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedhost_xforwardedproto_and_xforwardedport(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) = crate::host_meta_crawling();
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new("x-forwarded-host", "prevant.com"))
                .header(rocket::http::Header::new("x-forwarded-proto", "http"))
                .header(rocket::http::Header::new("x-forwarded-port", "8433"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();

            assert_json_include!(actual: value_in_json, expected: json!({
             "master": [
                    {
                     "url":"http://prevant.com:8433/master/service-a/"
                    }
                ]
            }));

            Ok(())
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedproto_and_other_default_values(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) = crate::host_meta_crawling();
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new("x-forwarded-proto", "https"))
                .header(rocket::http::Header::new("host", "localhost"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();
            assert_json_include!(actual: value_in_json, expected: json!({
             "master": [
                    {
                     "url":"https://localhost/master/service-a/"
                    }
                ]
            }));

            Ok(())
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedhost_and_other_default_values(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) = crate::host_meta_crawling();
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new("x-forwarded-host", "prevant.com"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();
            assert_json_include!(actual: value_in_json, expected: json!({
             "master": [
                    {
                     "url":"http://prevant.com/master/service-a/"
                    }
                ]
            }));

            Ok(())
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedport_and_default_values(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) = crate::host_meta_crawling();
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new("host", "localhost"))
                .header(rocket::http::Header::new("x-forwarded-port", "8433"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();
            assert_json_include!(actual: value_in_json, expected: json!({
             "master": [
                    {
                     "url":"http://localhost:8433/master/service-a/"
                    }
                ]
            }));

            Ok(())
        }

        #[tokio::test]
        async fn host_header_response_with_all_default_values(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) = crate::host_meta_crawling();
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());
            let get = client
                .get("/")
                .header(rocket::http::Header::new("host", "localhost"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();
            assert_json_include!(actual: value_in_json, expected: json!({
             "master": [
                    {
                     "url":"http://localhost/master/service-a/"
                    }
                ]
            }));

            Ok(())
        }

        #[tokio::test]
        async fn bad_request_without_host_header() {
            let (host_meta_cache, _host_meta_crawler) = crate::host_meta_crawling();
            let infrastructure = Box::new(Dummy::new());
            let apps = Arc::new(AppsService::new(Default::default(), infrastructure).unwrap());

            let rocket = rocket::build()
                .manage(host_meta_cache)
                .manage(apps)
                .mount("/", routes![crate::apps::routes::apps]);
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let mut get = client.get(rocket::uri!(crate::apps::routes::apps));
            get.add_header(ContentType::JSON);
            let response = get.dispatch().await;
            assert_eq!(response.status(), Status::BadRequest);
        }

        #[tokio::test]
        async fn with_invalid_headers() {
            let (host_meta_cache, _host_meta_crawler) = crate::host_meta_crawling();
            let infrastructure = Box::new(Dummy::new());
            let apps = Arc::new(AppsService::new(Default::default(), infrastructure).unwrap());

            let rocket = rocket::build()
                .manage(host_meta_cache)
                .manage(apps)
                .mount("/", routes![crate::apps::routes::apps]);
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get(rocket::uri!(crate::apps::routes::apps))
                .header(Header::new("x-forwarded-host", ""));

            let response = get.dispatch().await;
            assert_eq!(response.status(), Status::BadRequest);
        }

        #[tokio::test]
        async fn with_invalid_proto() {
            let (host_meta_cache, _host_meta_crawler) = crate::host_meta_crawling();
            let infrastructure = Box::new(Dummy::new());
            let apps = Arc::new(AppsService::new(Default::default(), infrastructure).unwrap());

            let rocket = rocket::build()
                .manage(host_meta_cache)
                .manage(apps)
                .mount("/", routes![crate::apps::routes::apps]);
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get(rocket::uri!(crate::apps::routes::apps))
                .header(Header::new("x-forwarded-proto", "."));

            let response = get.dispatch().await;
            assert_eq!(response.status(), Status::BadRequest);
        }
    }

    mod http_api_error {
        use super::super::*;
        use crate::{
            apps::{AppsError, AppsService},
            infrastructure::Dummy,
            registry::RegistryError,
        };
        use assert_json_diff::assert_json_eq;
        use rocket::{http::ContentType, local::asynchronous::Client};

        #[tokio::test]
        async fn invalid_service_payload() {
            let infrastructure = Box::new(Dummy::new());
            let apps = Arc::new(AppsService::new(Default::default(), infrastructure).unwrap());

            let rocket = rocket::build()
                .manage(apps)
                .mount("/", routes![crate::apps::routes::create_app]);

            let client = Client::tracked(rocket).await.expect("valid rocket");
            let response = client
                .post("/master")
                .body(
                    serde_json::json!([{
                        "serviceName": "db",
                        "image": "private-registry.example.com/_/postgres"
                    }])
                    .to_string(),
                )
                .header(ContentType::JSON)
                .dispatch()
                .await;

            assert_eq!(response.status(), Status::BadRequest);

            let body = response.into_string().await.unwrap();
            assert_json_eq!(
                serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({
                    "type": "https://httpstatuses.com/400",
                    "status": 400,
                    "title": "Bad Request",
                    "detail": "Invalid image: private-registry.example.com/_/postgres at line 1 column 51"
                })
            );
        }

        #[tokio::test]
        async fn image_registry_authentication_error() {
            #[get("/")]
            fn image_auth_failed() -> HttpResult<&'static str> {
                Err(AppsError::UnableToResolveImage {
                    error: RegistryError::AuthenticationFailure {
                        image: String::from("private-registry.example.com/_/postgres"),
                        failure: String::from("403: invalid user name and password"),
                    },
                }
                .into())
            }
            let rocket = rocket::build().mount("/", routes![image_auth_failed]);

            let client = Client::tracked(rocket).await.expect("valid rocket");
            let response = client.get("/").dispatch().await;

            assert_eq!(response.status(), Status::InternalServerError);

            let body = response.into_string().await.unwrap();
            assert_json_eq!(
                serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({
                    "type": "https://httpstatuses.com/500",
                    "status": 500,
                    "title": "Internal Server Error",
                    "detail": "Unable to resolve information about image: Cannot resolve image private-registry.example.com/_/postgres due to authentication failure: 403: invalid user name and password"
                })
            );
        }

        #[tokio::test]
        async fn image_registry_unexpected_error() {
            #[get("/")]
            fn image_not_found() -> HttpResult<&'static str> {
                Err(AppsError::UnableToResolveImage {
                    error: RegistryError::UnexpectedError {
                        image: String::from("private-registry.example.com/_/postgres"),
                        internal_message: String::from("unexpected"),
                    },
                }
                .into())
            }
            let rocket = rocket::build().mount("/", routes![image_not_found]);

            let client = Client::tracked(rocket).await.expect("valid rocket");
            let response = client.get("/").dispatch().await;

            assert_eq!(response.status(), Status::InternalServerError);

            let body = response.into_string().await.unwrap();
            assert_json_eq!(
                serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({
                    "type": "https://httpstatuses.com/500",
                    "status": 500,
                    "title": "Internal Server Error",
                    "detail": "Unable to resolve information about image: Unexpected docker registry error when resolving manifest for private-registry.example.com/_/postgres: unexpected"
                })
            );
        }

        #[tokio::test]
        async fn image_registry_not_found_error() {
            #[get("/")]
            fn image_not_found() -> HttpResult<&'static str> {
                Err(AppsError::UnableToResolveImage {
                    error: RegistryError::ImageNotFound {
                        image: String::from("private-registry.example.com/_/postgres"),
                    },
                }
                .into())
            }
            let rocket = rocket::build().mount("/", routes![image_not_found]);

            let client = Client::tracked(rocket).await.expect("valid rocket");
            let response = client.get("/").dispatch().await;

            assert_eq!(response.status(), Status::NotFound);

            let body = response.into_string().await.unwrap();
            assert_json_eq!(
                serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({
                    "type": "https://httpstatuses.com/404",
                    "status": 404,
                    "title": "Not Found",
                    "detail": "Unable to resolve information about image: Cannot find image private-registry.example.com/_/postgres"
                })
            );
        }
    }
}
