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

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate serde_derive;

use crate::apps::host_meta_crawling;
use crate::apps::Apps;
use crate::config::{Config, Runtime};
use crate::infrastructure::{Docker, Infrastructure, Kubernetes};
use crate::models::request_info::RequestInfo;
use auth::Auth;
use auth::Issuers;
use auth::User;
use clap::Parser;
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use http_result::HttpApiError;
use http_result::HttpResult;
use infrastructure::TraefikIngressRoute;
use rocket::fs::{FileServer, Options};
use rocket::routes;
use rocket::State;
use serde_yaml::{to_string, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt as _;

mod apps;
mod auth;
mod config;
mod deployment;
mod http_result;
mod infrastructure;
mod models;
mod registry;
mod tickets;
mod webhooks;

#[rocket::get("/")]
async fn openapi(request_info: RequestInfo) -> Option<String> {
    let openapi_path = Path::new("res").join("openapi.yml");
    let mut f = match File::open(openapi_path).await {
        Ok(f) => f,
        Err(e) => {
            log::error!("Cannot find API documentation: {}", e);
            return None;
        }
    };

    let mut contents = vec![];
    f.read_to_end(&mut contents).await.ok()?;
    let mut v: Value = serde_yaml::from_slice(&contents).ok()?;

    let mut url = request_info.base_url().clone();
    url.set_path("/api");
    v["servers"][0]["url"] = Value::String(url.to_string());

    Some(to_string(&v).unwrap())
}
#[derive(rocket::Responder)]
#[response(status = 200, content_type = "html")]
struct Index(String);

#[rocket::get("/")]
async fn index(user: User, issuers: &State<Issuers>) -> HttpResult<Index> {
    use handlebars::Handlebars;

    let index_path = Path::new("frontend").join("index.html");

    let mut f = File::open(index_path).await.map_err(|e| {
        HttpApiError::from(
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string()),
        )
    })?;

    let mut contents = String::new();
    f.read_to_string(&mut contents).await.map_err(|e| {
        HttpApiError::from(
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string()),
        )
    })?;

    let mut handlebars = Handlebars::new();
    handlebars.register_escape_fn(handlebars::no_escape);
    handlebars
        .register_template_string("index", contents)
        .map_err(|e| {
            HttpApiError::from(
                HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                    .detail(e.to_string()),
            )
        })?;

    let mut data = BTreeMap::new();
    let me = match user {
        User::Anonymous => serde_json::Value::Null,
        User::Oidc { sub, iss, name } => serde_json::json!({
            "sub": sub,
            "iss": iss,
            "name": name
        }),
    };
    data.insert("me", me.to_string());
    data.insert("issuers", issuers.inner().to_string());

    Ok(Index(handlebars.render("index", &data).map_err(|e| {
        HttpApiError::from(
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(e.to_string()),
        )
    })?))
}

fn create_infrastructure(config: &Config) -> Box<dyn Infrastructure> {
    match config.runtime_config() {
        Runtime::Docker => {
            log::info!("Using Docker backend");
            Box::new(Docker::new(config.clone()))
        }
        Runtime::Kubernetes(_config) => {
            log::info!("Using Kubernetes backend");
            Box::new(Kubernetes::new(config.clone()))
        }
    }
}

#[rocket::main]
async fn main() -> Result<(), StartUpError> {
    use base64::prelude::*;
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = crate::config::CliArgs::parse();

    let config = Config::from_figment(&cli).map_err(|err| StartUpError::InvalidConfiguration {
        err: err.to_string(),
    })?;

    if let Ok(rocket_config) = rocket::Config::figment().extract::<rocket::Config>() {
        if rocket_config.secret_key.is_zero() {
            if !config.api_access.openid_providers.is_empty() {
                log::warn!("Generating secret key for secure authentication. Please, set ROCKET_SECRET_KEY variable to make sure that logins survive restarts.");
            }

            let mut key = [0u8; 32];
            key[0..16].copy_from_slice(&uuid::Uuid::new_v4().to_bytes_le().as_slice());
            key[16..].copy_from_slice(&uuid::Uuid::new_v4().to_bytes_le().as_slice());
            std::env::set_var("ROCKET_SECRET_KEY", BASE64_STANDARD.encode(key));
        }
    }

    let infrastructure = create_infrastructure(&config);

    let prevant_base_route = if let Some(base_url) = &config.base_url {
        Some(TraefikIngressRoute::from(base_url))
    } else {
        infrastructure
            .base_traefik_ingress_route()
            .await
            .inspect_err(|err| log::info!("Cannot read base route from the infrastructure: {err}"))
            .ok()
            .flatten()
    };

    let apps = Apps::new(config.clone(), infrastructure)
        .map_err(|e| StartUpError::CannotCreateApps { err: e.to_string() })?
        .with_base_route(prevant_base_route.clone());

    // TODO: Every interaction with apps is blocked by the Arc. For example, the background job in
    // host_meta_crawler blocks every get request for the waiting time.
    // Arc<Apps> needs to be replace with Apps
    let apps = Arc::new(apps);

    let app_updates = apps.app_updates().await;

    let (host_meta_cache, host_meta_crawler) = host_meta_crawling(config.clone());
    host_meta_crawler.spawn(apps.clone(), app_updates.clone());

    let _rocket = rocket::build()
        .manage(config)
        .manage(prevant_base_route)
        .manage(apps)
        .manage(host_meta_cache)
        .manage(app_updates)
        .mount("/", routes![index])
        .mount(
            "/",
            FileServer::new(Path::new("frontend"), Options::Index | Options::Missing),
        )
        .mount("/openapi.yaml", routes![openapi])
        .mount("/api/apps", crate::apps::apps_routes())
        .mount("/api", routes![tickets::tickets])
        .mount("/api", routes![webhooks::webhooks])
        .mount("/auth", crate::auth::auth_routes())
        .attach(Auth::fairing())
        .launch()
        .await?;

    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum StartUpError {
    #[error("Cannot read configuration: {err}")]
    InvalidConfiguration { err: String },
    #[error("Cannot start HTTP server: {err}")]
    CannotStartWebServer { err: String },
    #[error("Cannot create apps service: {err}")]
    CannotCreateApps { err: String },
}

impl std::convert::From<rocket::Error> for StartUpError {
    fn from(err: rocket::Error) -> Self {
        Self::CannotStartWebServer {
            err: err.to_string(),
        }
    }
}
