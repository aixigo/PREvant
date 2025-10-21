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

use super::queue::AppTaskQueueProducer;
use crate::apps::{Apps, AppsError};
use crate::auth::UserValidatedByAccessMode;
use crate::config::Config;
use crate::deployment::hooks::Hooks;
use crate::http_result::{HttpApiError, HttpResult};
use crate::models::{
    App, AppName, AppNameError, AppStatusChangeId, AppStatusChangeIdError, Owner, Service,
    ServiceStatus,
};
use create_app_payload::CreateAppPayload;
use http_api_problem::{HttpApiProblem, StatusCode};
use log::error;
use regex::Regex;
use rocket::http::Status;
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::{Responder, Response};
use rocket::serde::json::Json;
use rocket::{FromForm, State};
use serde::ser::{Serialize, SerializeSeq};
use serde::Serializer;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

mod create_app_payload;
mod get_apps;
mod logs;
mod static_openapi_spec;

pub fn apps_routes() -> Vec<rocket::Route> {
    rocket::routes![
        get_apps::apps_v1,
        get_apps::apps_v2,
        get_apps::stream_apps_v1,
        get_apps::stream_apps_v2,
        delete_app_v1,
        delete_app_v2,
        create_app_v1,
        create_app_v2,
        logs::logs,
        logs::stream_logs,
        change_status,
        status_change_v1,
        status_change_v2,
        static_openapi_spec::static_open_api_spec,
    ]
}

pub struct AppV1(App);

impl Serialize for AppV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.0.services().len()))?;

        for service in self.0.services() {
            seq.serialize_element(service)?;
        }

        serde::ser::SerializeSeq::end(seq)
    }
}

pub struct AppV2(App);

impl Serialize for AppV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct App<'a> {
            services: &'a [Service],
            #[serde(skip_serializing_if = "HashSet::is_empty")]
            owners: &'a HashSet<Owner>,
        }

        let app = App {
            services: self.0.services(),
            owners: self.0.owners(),
        };

        app.serialize(serializer)
    }
}

#[rocket::get(
    "/<app_name>/status-changes/<status_id>",
    format = "application/json",
    rank = 1
)]
async fn status_change_v1(
    app_name: Result<AppName, AppNameError>,
    status_id: Result<AppStatusChangeId, AppStatusChangeIdError>,
    app_queue: &State<AppTaskQueueProducer>,
    options: WaitForQueueOptions,
) -> HttpResult<AsyncCompletion<Json<AppV1>>> {
    let app_name = app_name?;
    let status_id = status_id?;

    try_wait_for_task(app_queue, app_name, status_id, options, AppV1).await
}

#[rocket::get(
    "/<app_name>/status-changes/<status_id>",
    format = "application/vnd.prevant.v2+json",
    rank = 2
)]
async fn status_change_v2(
    app_name: Result<AppName, AppNameError>,
    status_id: Result<AppStatusChangeId, AppStatusChangeIdError>,
    app_queue: &State<AppTaskQueueProducer>,
    options: WaitForQueueOptions,
) -> HttpResult<AsyncCompletion<Json<AppV2>>> {
    let app_name = app_name?;
    let status_id = status_id?;

    try_wait_for_task(app_queue, app_name, status_id, options, AppV2).await
}

async fn delete_app<F, R>(
    app_name: Result<AppName, AppNameError>,
    app_queue: &State<AppTaskQueueProducer>,
    options: WaitForQueueOptions,
    user: Result<UserValidatedByAccessMode, HttpApiProblem>,
    create_return_value: F,
) -> HttpResult<AsyncCompletion<Json<R>>>
where
    F: FnOnce(App) -> R,
{
    // TODO: authorization hook to verify e.g. if a user is member of a GitLab group
    let _user = user.map_err(HttpApiError::from)?;

    let app_name = app_name?;

    let status_id = app_queue
        .enqueue_delete_task(app_name.clone())
        .await
        .map_err(|e| {
            HttpApiError::from(
                HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                    .detail(e.to_string()),
            )
        })?;

    try_wait_for_task(app_queue, app_name, status_id, options, create_return_value).await
}

#[rocket::delete("/<app_name>", rank = 1)]
pub async fn delete_app_v1(
    app_name: Result<AppName, AppNameError>,
    app_queue: &State<AppTaskQueueProducer>,
    options: WaitForQueueOptions,
    user: Result<UserValidatedByAccessMode, HttpApiProblem>,
) -> HttpResult<AsyncCompletion<Json<AppV1>>> {
    delete_app(app_name, app_queue, options, user, AppV1).await
}

#[rocket::delete("/<app_name>", format = "application/vnd.prevant.v2+json", rank = 2)]
pub async fn delete_app_v2(
    app_name: Result<AppName, AppNameError>,
    app_queue: &State<AppTaskQueueProducer>,
    options: WaitForQueueOptions,
    user: Result<UserValidatedByAccessMode, HttpApiProblem>,
) -> HttpResult<AsyncCompletion<Json<AppV2>>> {
    delete_app(app_name, app_queue, options, user, AppV2).await
}

pub async fn delete_app_sync(
    app_name: Result<AppName, AppNameError>,
    app_queue: &State<AppTaskQueueProducer>,
    user: Result<UserValidatedByAccessMode, HttpApiProblem>,
) -> HttpResult<Json<AppV1>> {
    match delete_app_v1(app_name, app_queue, WaitForQueueOptions::Sync, user).await? {
        AsyncCompletion::Pending(_, _) => {
            Err(HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR).into())
        }
        AsyncCompletion::Ready(result) => Ok(result),
    }
}

async fn create_app<F, R>(
    app_name: Result<AppName, AppNameError>,
    app_queue: &State<AppTaskQueueProducer>,
    create_app_form: CreateAppOptions,
    payload: Result<CreateAppPayload, HttpApiProblem>,
    options: WaitForQueueOptions,
    hooks: &Hooks<'_>,
    user: Result<UserValidatedByAccessMode, HttpApiProblem>,
    create_return_value: F,
) -> HttpResult<AsyncCompletion<Json<R>>>
where
    F: FnOnce(App) -> R,
{
    let payload = payload.map_err(HttpApiError::from)?;
    // TODO: authorization hook to verify e.g. if a user is member of a GitLab group
    let user = user.map_err(HttpApiError::from)?;

    let app_name = app_name?;
    let replicate_from = create_app_form.replicate_from().clone();

    let owner = hooks
        .apply_id_token_claims_to_owner_hook(user.user)
        .await
        .map_err(|e| {
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string())
        })?;

    let status_id = app_queue
        .enqueue_create_or_update_task(
            app_name.clone(),
            replicate_from,
            payload.services,
            owner,
            payload.user_defined_parameters,
        )
        .await
        .map_err(|e| {
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string())
        })?;

    try_wait_for_task(app_queue, app_name, status_id, options, create_return_value).await
}

#[rocket::post(
    "/<app_name>?<create_app_form..>",
    format = "application/json",
    data = "<payload>",
    rank = 1
)]
pub async fn create_app_v1(
    app_name: Result<AppName, AppNameError>,
    app_queue: &State<AppTaskQueueProducer>,
    create_app_form: CreateAppOptions,
    payload: Result<CreateAppPayload, HttpApiProblem>,
    options: WaitForQueueOptions,
    hooks: Hooks<'_>,
    user: Result<UserValidatedByAccessMode, HttpApiProblem>,
) -> HttpResult<AsyncCompletion<Json<AppV1>>> {
    create_app(
        app_name,
        app_queue,
        create_app_form,
        payload,
        options,
        &hooks,
        user,
        AppV1,
    )
    .await
}

#[rocket::post(
    "/<app_name>?<create_app_form..>",
    format = "application/vnd.prevant.v2+json",
    data = "<payload>",
    rank = 2
)]
pub async fn create_app_v2(
    app_name: Result<AppName, AppNameError>,
    app_queue: &State<AppTaskQueueProducer>,
    create_app_form: CreateAppOptions,
    payload: Result<CreateAppPayload, HttpApiProblem>,
    options: WaitForQueueOptions,
    hooks: Hooks<'_>,
    user: Result<UserValidatedByAccessMode, HttpApiProblem>,
) -> HttpResult<AsyncCompletion<Json<AppV2>>> {
    create_app(
        app_name,
        app_queue,
        create_app_form,
        payload,
        options,
        &hooks,
        user,
        AppV2,
    )
    .await
}

#[rocket::put(
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
pub enum WaitForQueueOptions {
    Sync,
    Async { wait: Option<Duration> },
}

pub async fn try_wait_for_task<F, R>(
    app_queue: &State<AppTaskQueueProducer>,
    app_name: AppName,
    status_id: AppStatusChangeId,
    options: WaitForQueueOptions,
    create_return_value: F,
) -> HttpResult<AsyncCompletion<Json<R>>>
where
    F: FnOnce(App) -> R,
{
    let wait = match options {
        WaitForQueueOptions::Sync => Duration::from_secs(60 * 5 * 60),
        WaitForQueueOptions::Async { wait } => wait.unwrap_or(Duration::from_secs(10)),
    };

    match app_queue.try_wait_for_task(&status_id, wait).await {
        Some(Ok(app)) => Ok(AsyncCompletion::Ready(Json(create_return_value(app)))),
        Some(Err(err)) => Err(err.into()),
        None => Ok(AsyncCompletion::Pending(app_name, status_id)),
    }
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
                let url = format!("/api/apps/{app_name}/status-changes/{status_id}");
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
            AppsError::InvalidUserDefinedParameters { .. } => StatusCode::BAD_REQUEST,
            AppsError::AppLimitExceeded { .. } => StatusCode::PRECONDITION_FAILED,
            AppsError::UnableToResolveImage { error } => match **error {
                crate::registry::RegistryError::ImageNotFound { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            AppsError::AppNotFound { .. } => StatusCode::NOT_FOUND,
            AppsError::InfrastructureError { .. }
            | AppsError::InvalidServerConfiguration { .. }
            | AppsError::TemplatingIssue { .. }
            | AppsError::UnapplicableHook { .. } => {
                error!("Internal server error: {error}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };

        HttpApiProblem::with_title_and_type(status)
            .detail(format!("{error}"))
            .into()
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for WaitForQueueOptions {
    type Error = &'static str;

    async fn from_request(request: &'r Request<'_>) -> rocket::request::Outcome<Self, Self::Error> {
        let headers = request.headers().get("Prefer").collect::<Vec<_>>();

        let mut wait_options = WaitForQueueOptions::Sync;
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
                wait_options = WaitForQueueOptions::Async { wait: None };
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

        Outcome::Success(match (wait_options, wait) {
            (WaitForQueueOptions::Sync, _) => WaitForQueueOptions::Sync,
            (WaitForQueueOptions::Async { .. }, wait) => WaitForQueueOptions::Async { wait },
        })
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for Hooks<'r> {
    type Error = HttpApiProblem;
    async fn from_request(request: &'r Request<'_>) -> rocket::request::Outcome<Self, Self::Error> {
        let Some(config) = request.rocket().state::<Config>() else {
            return rocket::request::Outcome::Error((
                Status::InternalServerError,
                HttpApiProblem::with_title_and_type(StatusCode::BAD_REQUEST),
            ));
        };

        let hooks = Hooks::new(config);
        return rocket::request::Outcome::Success(hooks);
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        apps::routes::{AppV1, AppV2},
        models::{App, Owner, Service, ServiceStatus},
    };
    use assert_json_diff::assert_json_eq;
    use chrono::Utc;
    use openidconnect::{IssuerUrl, SubjectIdentifier};
    use std::collections::HashSet;

    mod parse_wait_for_queue_options_from_request {
        use crate::apps::routes::*;
        use rocket::http::Header;
        use rocket::local::asynchronous::Client;

        #[tokio::test]
        async fn without_prefer_header() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client.get("/");
            let request = get.inner();

            let wait_for_queue_options =
                WaitForQueueOptions::from_request(request).await.succeeded();

            assert_eq!(wait_for_queue_options, Some(WaitForQueueOptions::Sync));
        }

        #[tokio::test]
        async fn with_unknown_prefer_header_content() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get("/")
                .header(Header::new("Prefer", "handling=lenient"));
            let request = get.inner();

            let wait_for_queue_options =
                WaitForQueueOptions::from_request(request).await.succeeded();

            assert_eq!(wait_for_queue_options, Some(WaitForQueueOptions::Sync));
        }

        #[tokio::test]
        async fn prefer_async_without_wait() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get("/")
                .header(Header::new("Prefer", "respond-async"));
            let request = get.inner();

            let wait_for_queue_options =
                WaitForQueueOptions::from_request(request).await.succeeded();

            assert_eq!(
                wait_for_queue_options,
                Some(WaitForQueueOptions::Async { wait: None })
            );
        }

        #[tokio::test]
        async fn prefer_async_with_wait() {
            let rocket = rocket::build();
            let client = Client::tracked(rocket).await.expect("valid rocket");
            let get = client
                .get("/")
                .header(Header::new("Prefer", "respond-async, wait=100"));
            let request = get.inner();

            let wait_for_queue_options =
                WaitForQueueOptions::from_request(request).await.succeeded();

            assert_eq!(
                wait_for_queue_options,
                Some(WaitForQueueOptions::Async {
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

            let wait_for_queue_options =
                WaitForQueueOptions::from_request(request).await.succeeded();

            assert_eq!(
                wait_for_queue_options,
                Some(WaitForQueueOptions::Async {
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

            let wait_for_queue_options =
                WaitForQueueOptions::from_request(request).await.succeeded();

            assert_eq!(wait_for_queue_options, Some(WaitForQueueOptions::Sync));
        }
    }

    mod http_api_error {
        use super::super::*;
        use crate::{
            apps::{AppProcessingQueue, AppsError, AppsService},
            infrastructure::Dummy,
            registry::RegistryError,
        };
        use assert_json_diff::assert_json_eq;
        use rocket::{http::ContentType, local::asynchronous::Client, routes};

        #[tokio::test]
        async fn invalid_service_payload() {
            let infrastructure = Box::new(Dummy::new());
            let apps = Arc::new(AppsService::new(Default::default(), infrastructure).unwrap());

            let rocket = rocket::build()
                .manage(apps)
                .manage(Config::default())
                .mount("/", routes![crate::apps::routes::create_app_v1])
                .attach(AppProcessingQueue::fairing());

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
                    "detail": "Invalid image: private-registry.example.com/_/postgres"
                })
            );
        }

        #[rocket::get("/")]
        fn image_auth_failed() -> HttpResult<&'static str> {
            Err(AppsError::UnableToResolveImage {
                error: Arc::new(RegistryError::AuthenticationFailure {
                    image: String::from("private-registry.example.com/_/postgres"),
                    failure: String::from("403: invalid user name and password"),
                }),
            }
            .into())
        }

        #[tokio::test]
        async fn image_registry_authentication_error() {
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

        #[rocket::get("/")]
        fn registry_unexpected() -> HttpResult<&'static str> {
            Err(AppsError::UnableToResolveImage {
                error: Arc::new(RegistryError::UnexpectedError {
                    image: String::from("private-registry.example.com/_/postgres"),
                    err: anyhow::Error::msg("unexpected"),
                }),
            }
            .into())
        }

        #[tokio::test]
        async fn image_registry_unexpected_error() {
            let rocket = rocket::build().mount("/", routes![registry_unexpected]);

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

        #[rocket::get("/")]
        fn image_not_found() -> HttpResult<&'static str> {
            Err(AppsError::UnableToResolveImage {
                error: Arc::new(RegistryError::ImageNotFound {
                    image: String::from("private-registry.example.com/_/postgres"),
                }),
            }
            .into())
        }

        #[tokio::test]
        async fn image_registry_not_found_error() {
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

    mod deployment_with_additional_client_parameters {
        use super::super::*;
        use crate::{
            apps::{AppProcessingQueue, AppsService},
            config_from_str,
            infrastructure::Dummy,
        };
        use assert_json_diff::assert_json_include;
        use rocket::{http::ContentType, local::asynchronous::Client, routes};

        macro_rules! config_from_str {
            ( $config_str:expr_2021 ) => {
                toml::from_str::<crate::config::Config>($config_str).unwrap()
            };
        }

        async fn create_client() -> Client {
            let config = config_from_str!(
                r#"
                    [companions.adminer]
                    serviceName = 'adminer{{#if userDefined}}-{{userDefined.test}}{{/if}}'
                    type = 'application'
                    image = 'adminer:4.8.1'

                    [companions.templating.userDefinedSchema]
                    type = "object"
                    properties = { test = { type = "string" }  }
                "#
            );

            let infrastructure = Box::new(Dummy::new());
            let apps = Arc::new(AppsService::new(config.clone(), infrastructure).unwrap());

            let rocket = rocket::build()
                .manage(apps)
                .manage(config)
                .mount("/", routes![crate::apps::routes::create_app_v1])
                .attach(AppProcessingQueue::fairing());

            Client::tracked(rocket).await.expect("valid rocket")
        }

        #[tokio::test]
        async fn without_user_defined_payload() {
            let client = create_client().await;

            let response = client
                .post("/master")
                .body(
                    serde_json::json!(
                        [{
                            "serviceName": "db",
                            "image": "postgres"
                        }]
                    )
                    .to_string(),
                )
                .header(ContentType::JSON)
                .dispatch()
                .await;

            assert_eq!(response.status(), Status::Ok);

            let body = response.into_string().await.unwrap();
            assert_json_include!(
                actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                expected: serde_json::json!([{
                    "name": "adminer",
                }, {
                    "name": "db",
                }])
            );
        }

        #[tokio::test]
        async fn with_invalid_user_defined_payload() {
            let client = create_client().await;

            let response = client
                .post("/master")
                .body(
                    serde_json::json!({
                        "services": [{
                            "serviceName": "db",
                            "image": "postgres"
                        }],
                        "userDefined": "test"
                    })
                    .to_string(),
                )
                .header(ContentType::JSON)
                .dispatch()
                .await;

            assert_eq!(response.status(), Status::BadRequest);

            let body = response.into_string().await.unwrap();
            assert_json_include!(
                actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                expected: serde_json::json!({
                    "status": 400,
                    "detail": "User defined payload does not match to the configured value: Provided data (\"test\") does not match schema: \"test\" is not of type \"object\""
                })
            );
        }

        #[tokio::test]
        async fn with_user_defined_payload() {
            let client = create_client().await;

            let response = client
                .post("/master")
                .body(
                    serde_json::json!({
                        "services": [{
                            "serviceName": "db",
                            "image": "postgres"
                        }],
                        "userDefined": { "test": "ud" }
                    })
                    .to_string(),
                )
                .header(ContentType::JSON)
                .dispatch()
                .await;

            assert_eq!(response.status(), Status::Ok);

            let body = response.into_string().await.unwrap();
            assert_json_include!(
                actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                expected: serde_json::json!([{
                    "name": "adminer-ud",
                }, {
                    "name": "db",
                }])
            );
        }

        #[tokio::test]
        async fn with_user_defined_payload_and_without_services() {
            let client = create_client().await;

            let response = client
                .post("/master")
                .body(
                    serde_json::json!({
                        "userDefined": { "test": "ud" }
                    })
                    .to_string(),
                )
                .header(ContentType::JSON)
                .dispatch()
                .await;

            assert_eq!(response.status(), Status::Ok);

            let body = response.into_string().await.unwrap();
            assert_json_include!(
                actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                expected: serde_json::json!([{
                    "name": "adminer-ud",
                }])
            );
        }
    }

    #[test]
    fn serialize_app_v1() {
        assert_json_eq!(
            serde_json::json!([{
                "name": "mariadb",
                "type": "instance",
                "state": {
                    "status": "running"
                }
            }, {
                "name": "postgres",
                "type": "instance",
                "state": {
                    "status": "running"
                }
            }]),
            serde_json::to_value(AppV1(App::new(
                vec![
                    Service {
                        id: String::from("some id"),
                        state: crate::models::State {
                            status: ServiceStatus::Running,
                            started_at: Some(Utc::now()),
                        },
                        config: crate::sc!("postgres", "postgres:latest")
                    },
                    Service {
                        id: String::from("some id"),
                        state: crate::models::State {
                            status: ServiceStatus::Running,
                            started_at: Some(Utc::now()),
                        },
                        config: crate::sc!("mariadb", "mariadb:latest")
                    }
                ],
                HashSet::new(),
                None
            )))
            .unwrap()
        );
    }

    #[test]
    fn serialize_app_v2() {
        assert_json_eq!(
            serde_json::json!({
                "services": [{
                    "name": "mariadb",
                    "type": "instance",
                    "state": {
                        "status": "running"
                    }
                }, {
                    "name": "postgres",
                    "type": "instance",
                    "state": {
                        "status": "running"
                    }
                }],
                "owners": [{
                    "sub": "some-sub",
                    "iss": "https://openid.example.com"
                }],
            }),
            serde_json::to_value(AppV2(App::new(
                vec![
                    Service {
                        id: String::from("some id"),
                        state: crate::models::State {
                            status: ServiceStatus::Running,
                            started_at: Some(Utc::now()),
                        },
                        config: crate::sc!("postgres", "postgres:latest")
                    },
                    Service {
                        id: String::from("some id"),
                        state: crate::models::State {
                            status: ServiceStatus::Running,
                            started_at: Some(Utc::now()),
                        },
                        config: crate::sc!("mariadb", "mariadb:latest")
                    }
                ],
                HashSet::from([Owner {
                    sub: SubjectIdentifier::new(String::from("some-sub")),
                    iss: IssuerUrl::new(String::from("https://openid.example.com")).unwrap(),
                    name: None,
                }]),
                None
            )))
            .unwrap()
        );
    }

    mod basic_deployment {
        use crate::{
            apps::{AppProcessingQueue, AppsService},
            config_from_str,
            infrastructure::Dummy,
        };
        use assert_json_diff::assert_json_include;
        use http::header::LOCATION;
        use rocket::{
            http::{Accept, ContentType, Header, Status},
            local::asynchronous::Client,
            routes,
        };
        use std::{sync::Arc, time::Duration};

        async fn create_client() -> Client {
            let config = config_from_str!("");

            let infrastructure = Box::new(Dummy::with_delay(Duration::from_secs(5)));
            let apps = Arc::new(AppsService::new(config.clone(), infrastructure).unwrap());

            let rocket = rocket::build()
                .manage(config)
                .manage(apps)
                .mount(
                    "/",
                    routes![
                        crate::apps::routes::create_app_v1,
                        crate::apps::routes::delete_app_v1,
                        crate::apps::routes::status_change_v1,
                    ],
                )
                .attach(AppProcessingQueue::fairing());

            Client::tracked(rocket).await.expect("valid rocket")
        }

        #[tokio::test]
        async fn deploy_services_and_respond_with_deployed_client() {
            let client = create_client().await;

            let response = client
                .post("/master")
                .body(
                    serde_json::json!({
                        "services": [{
                            "serviceName": "db",
                            "image": "postgres"
                        }],
                    })
                    .to_string(),
                )
                .header(ContentType::JSON)
                .dispatch()
                .await;

            assert_eq!(response.status(), Status::Ok);

            let body = response.into_string().await.unwrap();
            assert_json_include!(
                actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                expected: serde_json::json!([{
                    "name": "db",
                }])
            );
        }

        #[tokio::test]
        async fn deploy_services_and_respond_with_deployed_client_with_http_async() {
            let client = create_client().await;

            let response = client
                .post("/master")
                .body(
                    serde_json::json!({
                        "services": [{
                            "serviceName": "db",
                            "image": "postgres"
                        }],
                    })
                    .to_string(),
                )
                .header(ContentType::JSON)
                .header(Header::new("Prefer", "respond-async,wait=1"))
                .dispatch()
                .await;

            assert_eq!(response.status(), Status::Accepted);
            let location = response
                .headers()
                .get(LOCATION.as_str())
                .next()
                .unwrap()
                .trim_start_matches("/api/apps");

            let mut polls = 0;
            let body = loop {
                let response = client
                    .get(location)
                    .header(Accept::JSON)
                    .header(Header::new("Prefer", "respond-async,wait=1"))
                    .dispatch()
                    .await;

                polls += 1;
                assert!(polls <= 10);

                if response.status() == Status::Ok {
                    let body = response.into_string().await.unwrap();
                    break body;
                }
            };

            assert!(polls > 0);
            assert_json_include!(
                actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                expected: serde_json::json!([{
                    "name": "db",
                }])
            );
        }

        #[tokio::test]
        async fn deploy_services_and_delete_services() {
            let client = create_client().await;

            let response = client
                .post("/master")
                .body(
                    serde_json::json!({
                        "services": [{
                            "serviceName": "db",
                            "image": "postgres"
                        }],
                    })
                    .to_string(),
                )
                .header(ContentType::JSON)
                .dispatch()
                .await;

            assert_eq!(response.status(), Status::Ok);

            let response = client.delete("/master").dispatch().await;

            assert_eq!(response.status(), Status::Ok);
        }
    }
}
