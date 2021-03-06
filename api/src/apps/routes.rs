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
use crate::models::{AppName, AppNameError, LogChunk};
use crate::models::{AppStatusChangeId, AppStatusChangeIdError};
use chrono::DateTime;
use http_api_problem::{HttpApiProblem, StatusCode};
use multimap::MultiMap;
use regex::Regex;
use rocket::http::{RawStr, Status};
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::{Responder, Response};
use rocket::serde::json::Json;
use rocket::State;
use std::future::Future;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::time::timeout;

pub fn apps_routes() -> Vec<rocket::Route> {
    rocket::routes![
        apps,
        delete_app,
        create_app,
        logs,
        change_status,
        status_change
    ]
}

#[get("/", format = "application/json")]
async fn apps(
    apps: &State<Arc<Apps>>,
    request_info: RequestInfo,
    host_meta_cache: &State<HostMetaCache>,
) -> HttpResult<Json<MultiMap<String, Service>>> {
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
    service_configs: Json<Vec<ServiceConfig>>,
    options: RunOptions,
) -> HttpResult<AsyncCompletion<Json<Vec<Service>>>> {
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

#[get(
    "/<app_name>/logs/<service_name>?<since>&<limit>",
    format = "text/plain"
)]
async fn logs(
    app_name: Result<AppName, AppNameError>,
    service_name: String,
    since: Option<String>,
    limit: Option<usize>,
    apps: &State<Arc<Apps>>,
) -> HttpResult<LogsResponse> {
    let app_name = app_name?;

    let since = match since {
        None => None,
        Some(since) => match DateTime::parse_from_rfc3339(&since) {
            Ok(since) => Some(since),
            Err(err) => {
                return Err(
                    HttpApiProblem::with_title(http_api_problem::StatusCode::BAD_REQUEST)
                        .detail(format!("{}", err))
                        .into(),
                );
            }
        },
    };
    let limit = limit.unwrap_or(20_000);

    let log_chunk = apps
        .get_logs(&app_name, &service_name, &since, limit)
        .await?;

    Ok(LogsResponse {
        log_chunk,
        app_name,
        service_name,
        limit,
    })
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

pub struct LogsResponse {
    log_chunk: Option<LogChunk>,
    app_name: AppName,
    service_name: String,
    limit: usize,
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

impl<'r> Responder<'r, 'static> for LogsResponse {
    fn respond_to(self, _request: &'r Request) -> Result<Response<'static>, Status> {
        use std::io::Cursor;
        let log_chunk = match self.log_chunk {
            None => {
                let payload = HttpApiProblem::with_title(http_api_problem::StatusCode::NOT_FOUND)
                    .json_bytes();
                return Response::build()
                    .status(Status::NotFound)
                    .raw_header("Content-type", "application/problem+json")
                    .sized_body(payload.len(), Cursor::new(payload))
                    .ok();
            }
            Some(log_chunk) => log_chunk,
        };

        let from = *log_chunk.until() + chrono::Duration::milliseconds(1);

        let next_logs_url = format!(
            "/api/apps/{}/logs/{}/?limit={}&since={}",
            self.app_name,
            self.service_name,
            self.limit,
            RawStr::new(&from.to_rfc3339()).percent_encode(),
        );

        let log_lines = log_chunk.log_lines();
        Response::build()
            .raw_header("Link", format!("<{}>;rel=next", next_logs_url))
            .sized_body(log_lines.len(), Cursor::new(log_lines.clone()))
            .ok()
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
        let status = match error {
            AppsError::AppNotFound { .. } => StatusCode::NOT_FOUND,
            AppsError::AppIsInDeployment { .. } => StatusCode::CONFLICT,
            AppsError::AppIsInDeletion { .. } => StatusCode::CONFLICT,
            AppsError::InfrastructureError { .. }
            | AppsError::InvalidServerConfiguration { .. }
            | AppsError::InvalidTemplateFormat { .. }
            | AppsError::UnableToResolveImage { .. }
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
            .map(|header| header.split(","))
            .flatten()
            .map(str::trim)
        {
            if header == "respond-async" {
                run_options = RunOptions::Async { wait: None };
                continue;
            }

            if let Some(wait_capture) = RE.captures(header) {
                wait = Some(Duration::from_secs(
                    dbg!(wait_capture.get(1))
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
}
