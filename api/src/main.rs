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
extern crate clap;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate rocket;
#[macro_use]
extern crate serde_derive;

use crate::apps::host_meta_crawling;
use crate::apps::Apps;
use crate::config::{Config, Runtime};
use crate::infrastructure::{Docker, Infrastructure, Kubernetes};
use crate::models::request_info::RequestInfo;
use clap::{App, Arg};
use env_logger::Env;
use openssl::x509::X509;
use rocket::fs::NamedFile;
use secstr::SecUtf8;
use serde_yaml::{from_reader, to_string, Value};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;
use std::str::FromStr;
use std::sync::Arc;
use url::Url;

mod apps;
mod config;
mod http_result;
mod infrastructure;
mod models;
mod registry;
mod tickets;
mod webhooks;

#[get("/")]
async fn index() -> Option<NamedFile> {
    NamedFile::open(Path::new("frontend/index.html")).await.ok()
}

#[get("/<path..>", rank = 100)]
async fn files(path: PathBuf) -> Option<NamedFile> {
    NamedFile::open(Path::new("frontend/").join(path))
        .await
        .ok()
}

#[get("/")]
fn openapi(request_info: RequestInfo) -> Option<String> {
    let mut f = match File::open("openapi.yml") {
        Ok(f) => f,
        Err(e) => {
            error!("Cannot find API documentation: {}", e);
            return None;
        }
    };

    let mut v: Value = from_reader(&mut f).unwrap();

    let mut url = request_info.get_base_url().clone();
    url.set_path("/api");
    v["servers"][0]["url"] = Value::String(url.to_string());

    Some(to_string(&v).unwrap())
}

fn create_infrastructure(config: &Config) -> Result<Box<dyn Infrastructure>, StartUpError> {
    match config.runtime_config() {
        Runtime::Docker => Ok(Box::new(Docker::new())),
        Runtime::Kubernetes(kubernetes_config) => {
            let cluster_endpoint = match kubernetes_config.endpoint() {
                Some(endpoint) => endpoint.clone(),
                None => Url::from_str(&format!(
                    "https://{}:{}",
                    std::env::var("KUBERNETES_SERVICE_HOST").unwrap(),
                    std::env::var("KUBERNETES_SERVICE_PORT").unwrap()
                ))
                .unwrap(),
            };

            let cluster_ca = match kubernetes_config.cert_auth_file_path() {
                Some(path) => Some(read_ca_file(path)?),
                None => {
                    let container_secret =
                        Path::new("/run/secrets/kubernetes.io/serviceaccount/ca.crt");
                    if container_secret.exists() {
                        Some(read_ca_file(container_secret)?)
                    } else {
                        None
                    }
                }
            };

            let cluster_token = match kubernetes_config.token() {
                Some(token) => Some(token.clone()),
                None => {
                    let container_secret =
                        Path::new("/run/secrets/kubernetes.io/serviceaccount/token");
                    if container_secret.exists() {
                        Some(read_token_file(container_secret)?)
                    } else {
                        None
                    }
                }
            };

            Ok(Box::new(Kubernetes::new(
                cluster_endpoint,
                cluster_ca,
                cluster_token,
            )))
        }
    }
}

fn read_token_file(path: &Path) -> Result<SecUtf8, StartUpError> {
    debug!("Reading token from {}.", path.to_str().unwrap());

    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return Err(StartUpError::CannotReadToken {
                path: String::from(path.to_str().unwrap()),
                err: format!("{}", e),
            });
        }
    };

    let mut token = String::new();
    if let Err(e) = f.read_to_string(&mut token) {
        return Err(StartUpError::CannotReadToken {
            path: String::from(path.to_str().unwrap()),
            err: format!("{}", e),
        });
    }

    Ok(SecUtf8::from(token))
}

fn read_ca_file(path: &Path) -> Result<Vec<X509>, StartUpError> {
    debug!(
        "Reading certificate authority from {}.",
        path.to_str().unwrap()
    );

    let mut file =
        File::open(path).map_err(|err| StartUpError::CannotReadCertificateAuthority {
            path: String::from(path.to_str().unwrap()),
            err: format!("{}", err),
        })?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(|err| StartUpError::CannotReadCertificateAuthority {
            path: String::from(path.to_str().unwrap()),
            err: format!("{}", err),
        })?;

    X509::stack_from_pem(buffer.as_slice()).map_err(|err| {
        StartUpError::CannotReadCertificateAuthority {
            path: String::from(path.to_str().unwrap()),
            err: format!("{}", err),
        }
    })
}

#[rocket::main]
async fn main() -> Result<(), StartUpError> {
    let argument_matches = App::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!())
        .arg(
            Arg::with_name("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("The path to the configuration file")
                .takes_value(true),
        )
        .get_matches();

    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let config = match Config::load(argument_matches.value_of("config").unwrap_or("config.toml")) {
        Ok(config) => config,
        Err(e) => {
            error!("Cannot load config: {}", e);
            process::exit(0x0100);
        }
    };

    let infrastructure = create_infrastructure(&config)?;
    let apps = match Apps::new(config.clone(), infrastructure) {
        Ok(apps_service) => apps_service,
        Err(e) => {
            error!("Cannot create apps service: {}", e);
            process::exit(0x0200);
        }
    };

    let (host_meta_cache, host_meta_crawler) = host_meta_crawling();
    let apps = Arc::new(apps);
    host_meta_crawler.spawn(apps.clone());

    let _rocket = rocket::build()
        .manage(config)
        .manage(apps)
        .manage(host_meta_cache)
        .mount("/", routes![index])
        .mount("/openapi.yaml", routes![openapi])
        .mount("/", routes![files])
        .mount("/api/apps", crate::apps::apps_routes())
        .mount("/api", routes![tickets::tickets])
        .mount("/api", routes![webhooks::webhooks])
        .launch()
        .await?;

    Ok(())
}

#[derive(Debug, Fail)]
enum StartUpError {
    #[fail(display = "Cannot read certificate authority from {}: {}", path, err)]
    CannotReadCertificateAuthority { path: String, err: String },
    #[fail(display = "Cannot read API token from {}: {}", path, err)]
    CannotReadToken { path: String, err: String },
    #[fail(display = "Cannot start HTTP server: {}", err)]
    CannotStartWebServer { err: String },
}

impl std::convert::From<rocket::Error> for StartUpError {
    fn from(err: rocket::Error) -> Self {
        Self::CannotStartWebServer {
            err: err.to_string(),
        }
    }
}
