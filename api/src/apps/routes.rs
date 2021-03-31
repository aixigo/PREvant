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
use crate::apps::RUNTIME as runtime;
use crate::apps::{Apps, AppsError};
use crate::models::request_info::RequestInfo;
use crate::models::service::{Service, ServiceStatus};
use crate::models::ServiceConfig;
use crate::models::{AppName, AppNameError, LogChunk};
use crate::models::{AppStatusChangeId, AppStatusChangeIdError};
use chrono::DateTime;
use http_api_problem::{HttpApiProblem, StatusCode};
use hyper::header::Header;
use multimap::MultiMap;
use rocket::data::{self, FromDataSimple};
use rocket::http::hyper::header::{Location, Prefer, Preference};
use rocket::http::hyper::Error;
use rocket::http::{RawStr, Status};
use rocket::request::{Form, FromFormValue, FromRequest, Outcome, Request};
use rocket::response::{Responder, Response};
use rocket::Outcome::{Failure, Success};
use rocket::{Data, State};
use rocket_contrib::json::Json;
use std::future::Future;
use std::io::Read;
use std::str::FromStr;
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
fn apps(
    apps: State<Arc<Apps>>,
    request_info: RequestInfo,
    host_meta_cache: State<HostMetaCache>,
) -> Result<Json<MultiMap<String, Service>>, HttpApiProblem> {
    let services = runtime.block_on(apps.get_apps())?;
    Ok(Json(
        host_meta_cache.update_meta_data(services, &request_info),
    ))
}

#[get("/<app_name>/status-changes/<status_id>", format = "application/json")]
fn status_change(
    app_name: Result<AppName, AppNameError>,
    status_id: Result<AppStatusChangeId, AppStatusChangeIdError>,
    apps: State<Arc<Apps>>,
    options: RunOptions,
) -> Result<AsyncCompletion<Json<Vec<Service>>>, HttpApiProblem> {
    let app_name = app_name?;
    let status_id = status_id?;

    let apps = apps.clone();
    let future = async move { apps.wait_for_status_change(&status_id).await };

    match runtime.block_on(spawn_with_options(options, future))? {
        Poll::Pending => Ok(AsyncCompletion::Pending(app_name, status_id)),
        Poll::Ready(Ok(_)) => Err(HttpApiProblem::with_title_and_type_from_status(
            StatusCode::NOT_FOUND,
        )),
        Poll::Ready(Err(err)) => Err(HttpApiProblem::from(err)),
    }
}

#[delete("/<app_name>")]
pub fn delete_app(
    app_name: Result<AppName, AppNameError>,
    apps: State<Arc<Apps>>,
    options: RunOptions,
) -> Result<AsyncCompletion<Json<Vec<Service>>>, HttpApiProblem> {
    let app_name = app_name?;
    let app_name_cloned = app_name.clone();
    let status_id = AppStatusChangeId::new();

    let apps = apps.clone();
    let future = async move { apps.delete_app(&app_name, &status_id).await };

    match runtime.block_on(spawn_with_options(options, future))? {
        Poll::Pending => Ok(AsyncCompletion::Pending(app_name_cloned, status_id)),
        Poll::Ready(Ok(services)) => Ok(AsyncCompletion::Ready(Json(services))),
        Poll::Ready(Err(err)) => Err(HttpApiProblem::from(err)),
    }
}

pub fn delete_app_sync(
    app_name: Result<AppName, AppNameError>,
    apps: State<Arc<Apps>>,
) -> Result<Json<Vec<Service>>, HttpApiProblem> {
    match delete_app(app_name, apps, RunOptions::Sync)? {
        AsyncCompletion::Pending(_, _) => Err(HttpApiProblem::with_title_and_type_from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
        AsyncCompletion::Ready(result) => Ok(result),
    }
}

#[post(
    "/<app_name>?<create_app_form..>",
    format = "application/json",
    data = "<service_configs_data>"
)]
pub fn create_app(
    app_name: Result<AppName, AppNameError>,
    apps: State<Arc<Apps>>,
    create_app_form: Form<CreateAppOptions>,
    service_configs_data: ServiceConfigsData,
    options: RunOptions,
) -> Result<AsyncCompletion<Json<Vec<Service>>>, HttpApiProblem> {
    let status_id = AppStatusChangeId::new();
    let app_name = app_name?;
    let app_name_cloned = app_name.clone();
    let replicate_from = create_app_form.replicate_from().clone();
    let service_configs = service_configs_data.service_configs.clone();

    let apps = apps.clone();
    let future = async move {
        apps.create_or_update(
            &app_name.clone(),
            &status_id,
            replicate_from,
            &service_configs,
        )
        .await
    };

    match runtime.block_on(spawn_with_options(options, future))? {
        Poll::Pending => Ok(AsyncCompletion::Pending(app_name_cloned, status_id)),
        Poll::Ready(Ok(services)) => Ok(AsyncCompletion::Ready(Json(services))),
        Poll::Ready(Err(err)) => Err(HttpApiProblem::from(err)),
    }
}

#[put(
    "/<app_name>/states/<service_name>",
    format = "application/json",
    data = "<status_data>"
)]
fn change_status(
    app_name: Result<AppName, AppNameError>,
    service_name: String,
    apps: State<Arc<Apps>>,
    status_data: Json<ServiceStatusData>,
) -> Result<ServiceStatusResponse, HttpApiProblem> {
    let app_name = app_name?;
    let status = status_data.status.clone();

    let service = runtime.block_on(apps.change_status(&app_name, &service_name, status))?;

    Ok(ServiceStatusResponse { service })
}

#[get(
    "/<app_name>/logs/<service_name>?<since>&<limit>",
    format = "text/plain"
)]
fn logs(
    app_name: Result<AppName, AppNameError>,
    service_name: String,
    since: Option<String>,
    limit: Option<usize>,
    apps: State<Arc<Apps>>,
) -> Result<LogsResponse, HttpApiProblem> {
    let app_name = app_name?;

    let since = match since {
        None => None,
        Some(since) => match DateTime::parse_from_rfc3339(&since) {
            Ok(since) => Some(since),
            Err(err) => {
                return Err(HttpApiProblem::with_title_and_type_from_status(
                    http_api_problem::StatusCode::BAD_REQUEST,
                )
                .set_detail(format!("{}", err)));
            }
        },
    };
    let limit = limit.unwrap_or(20_000);

    let log_chunk = runtime.block_on(apps.get_logs(&app_name, &service_name, &since, limit))?;

    Ok(LogsResponse {
        log_chunk,
        app_name,
        service_name,
        limit,
    })
}

pub enum RunOptions {
    Sync,
    Async { wait: Option<Duration> },
}

pub async fn spawn_with_options<F>(
    options: RunOptions,
    future: F,
) -> Result<Poll<F::Output>, HttpApiProblem>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = runtime.handle();
    let join_handle = handle.spawn(future);

    match options {
        RunOptions::Sync => Ok(Poll::Ready(join_handle.await.map_err(map_join_error)?)),
        RunOptions::Async { wait: None } => Ok(Poll::Pending),
        RunOptions::Async {
            wait: Some(duration),
        } => {
            match handle
                .spawn(timeout(duration, join_handle))
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

fn map_join_error(err: tokio::task::JoinError) -> HttpApiProblem {
    HttpApiProblem::with_title_and_type_from_status(StatusCode::INTERNAL_SERVER_ERROR)
        .set_detail(format!("{}", err))
}

pub struct LogsResponse {
    log_chunk: Option<LogChunk>,
    app_name: AppName,
    service_name: String,
    limit: usize,
}

#[derive(FromForm)]
pub struct CreateAppOptions {
    #[form(field = "replicateFrom")]
    replicate_from: Option<AppName>,
}

impl CreateAppOptions {
    fn replicate_from(&self) -> &Option<AppName> {
        &self.replicate_from
    }
}

pub struct ServiceConfigsData {
    service_configs: Vec<ServiceConfig>,
}

impl FromDataSimple for ServiceConfigsData {
    type Error = String;

    fn from_data(_request: &Request, data: Data) -> data::Outcome<ServiceConfigsData, String> {
        let mut body = String::new();
        if let Err(e) = data.open().read_to_string(&mut body) {
            return Failure((Status::InternalServerError, format!("{:?}", e)));
        }

        let service_configs = match serde_json::from_str::<Vec<ServiceConfig>>(&body) {
            Ok(v) => v,
            Err(err) => {
                return Failure((
                    Status::BadRequest,
                    format!("Cannot read body as JSON: {:?}", err),
                ));
            }
        };

        Success(ServiceConfigsData { service_configs })
    }
}

impl Responder<'static> for LogsResponse {
    fn respond_to(self, _request: &Request) -> Result<Response<'static>, Status> {
        let log_chunk = match self.log_chunk {
            None => {
                return Ok(
                    HttpApiProblem::from(http_api_problem::StatusCode::NOT_FOUND)
                        .to_rocket_response(),
                )
            }
            Some(log_chunk) => log_chunk,
        };

        let from = *log_chunk.until() + chrono::Duration::milliseconds(1);

        let next_logs_url = format!(
            "/api/apps/{}/logs/{}/?limit={}&since={}",
            self.app_name,
            self.service_name,
            self.limit,
            rocket::http::uri::Uri::percent_encode(&from.to_rfc3339()),
        );
        Response::build()
            .raw_header("Link", format!("<{}>;rel=next", next_logs_url))
            .sized_body(std::io::Cursor::new(log_chunk.log_lines().clone()))
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

impl<'a, T: Responder<'a>> Responder<'a> for AsyncCompletion<T> {
    fn respond_to(self, request: &Request) -> Result<Response<'a>, Status> {
        match self {
            AsyncCompletion::Pending(app_name, status_id) => {
                let url = format!("/api/apps/{}/status-changes/{}", app_name, status_id);
                Response::build()
                    .status(Status::Accepted)
                    .header(Location(url))
                    .ok()
            }
            AsyncCompletion::Ready(result) => result.respond_to(request),
        }
    }
}

impl Responder<'static> for ServiceStatusResponse {
    fn respond_to(self, _request: &Request) -> Result<Response<'static>, Status> {
        match self.service {
            Some(_service) => Response::build().status(Status::Accepted).ok(),
            None => Response::build().status(Status::NotFound).ok(),
        }
    }
}

impl From<AppsError> for HttpApiProblem {
    fn from(error: AppsError) -> Self {
        let status = match error {
            AppsError::AppNotFound { .. } => StatusCode::NOT_FOUND,
            AppsError::AppIsInDeployment { .. } => StatusCode::CONFLICT,
            AppsError::AppIsInDeletion { .. } => StatusCode::CONFLICT,
            AppsError::InfrastructureError { .. }
            | AppsError::InvalidServerConfiguration { .. }
            | AppsError::InvalidTemplateFormat { .. }
            | AppsError::UnableToResolveImage { .. } => {
                error!("Internal server error: {}", error);
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };

        HttpApiProblem::with_title_and_type_from_status(status).set_detail(format!("{}", error))
    }
}

impl<'a> FromFormValue<'a> for AppName {
    type Error = String;

    fn from_form_value(form_value: &'a RawStr) -> Result<Self, Self::Error> {
        AppName::from_str(form_value).map_err(|e| format!("{}", e))
    }
}

impl<'a, 'r> FromRequest<'a, 'r> for RunOptions {
    type Error = hyper::Error;

    // Typed headers have been moved out of hyper into hyperium/headers.
    // When Rocket updates their hyper dependency, we probably have to update this.
    // See: https://github.com/SergioBenitez/Rocket/issues/1067
    fn from_request(request: &'a Request<'r>) -> Outcome<RunOptions, Error> {
        let headers = request
            .headers()
            .get(Prefer::header_name())
            .map(Vec::from)
            .collect::<Vec<Vec<u8>>>();

        if headers.is_empty() {
            return Outcome::Success(RunOptions::Sync);
        }

        match Prefer::parse_header(headers.as_slice()) {
            Err(err) => Outcome::Failure((Status::BadRequest, err)),
            Ok(prefer) => Outcome::Success({
                let mut sync = true;
                let mut wait = None;
                for preference in prefer.0 {
                    match preference {
                        Preference::RespondAsync => {
                            sync = false;
                        }
                        Preference::Wait(secs) => {
                            wait = Some(Duration::from_secs(u64::from(secs)));
                        }
                        _ => {}
                    }
                }
                match sync {
                    true => RunOptions::Sync,
                    false => RunOptions::Async { wait },
                }
            }),
        }
    }
}
