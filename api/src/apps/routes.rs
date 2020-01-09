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

use crate::apps::Apps;
use crate::models::request_info::RequestInfo;
use crate::models::service::{Service, ServiceStatus};
use crate::models::ServiceConfig;
use crate::models::{AppName, AppNameError, LogChunk};
use chrono::DateTime;
use http_api_problem::HttpApiProblem;
use multimap::MultiMap;
use rocket::data::{self, FromDataSimple};
use rocket::http::Status;
use rocket::request::{Form, Request};
use rocket::response::{Responder, Response};
use rocket::Outcome::{Failure, Success};
use rocket::{Data, State};
use rocket_contrib::json::Json;
use std::io::Read;

pub fn routes() -> Vec<rocket::Route> {
    rocket::routes![apps, delete_app, create_app, logs, change_status]
}

#[get("/", format = "application/json")]
fn apps(
    apps: State<Apps>,
    request_info: RequestInfo,
) -> Result<Json<MultiMap<String, Service>>, HttpApiProblem> {
    Ok(Json(apps.get_apps(&request_info)?))
}

#[delete("/<app_name>", format = "application/json")]
pub fn delete_app(
    app_name: Result<AppName, AppNameError>,
    apps: State<Apps>,
) -> Result<Json<Vec<Service>>, HttpApiProblem> {
    let app_name = app_name?;

    Ok(Json(apps.delete_app(&app_name)?))
}

#[post(
    "/<app_name>?<create_app_form..>",
    format = "application/json",
    data = "<service_configs_data>"
)]
fn create_app(
    app_name: Result<AppName, AppNameError>,
    apps: State<Apps>,
    create_app_form: Form<CreateAppOptions>,
    service_configs_data: ServiceConfigsData,
) -> Result<Json<Vec<Service>>, HttpApiProblem> {
    let app_name = app_name?;

    let services = apps.create_or_update(
        &app_name,
        create_app_form.replicate_from().clone(),
        &service_configs_data.service_configs,
    )?;

    Ok(Json(services))
}

#[put(
    "/<app_name>/states/<service_name>",
    format = "application/json",
    data = "<status_data>"
)]
fn change_status(
    app_name: Result<AppName, AppNameError>,
    service_name: String,
    apps: State<Apps>,
    status_data: Json<ServiceStatusData>,
) -> Result<ServiceStatusResponse, HttpApiProblem> {
    let app_name = app_name?;
    let status = status_data.status.clone();

    Ok(ServiceStatusResponse {
        service: apps.change_status(&app_name, &service_name, status)?,
    })
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
    apps: State<Apps>,
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

    let log_chunk = apps.get_logs(&app_name, &service_name, &since, limit)?;

    Ok(LogsResponse {
        log_chunk,
        app_name,
        service_name,
        limit,
    })
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
    replicate_from: Option<String>,
}

impl CreateAppOptions {
    fn replicate_from(&self) -> &Option<String> {
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

        let from = log_chunk.until().clone() + chrono::Duration::milliseconds(1);

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

impl Responder<'static> for ServiceStatusResponse {
    fn respond_to(self, _request: &Request) -> Result<Response<'static>, Status> {
        match self.service {
            Some(_service) => Response::build().status(Status::Accepted).ok(),
            None => Response::build().status(Status::NotFound).ok(),
        }
    }
}
