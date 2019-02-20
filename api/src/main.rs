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

#![feature(proc_macro_hygiene, decl_macro, try_from)]

#[macro_use]
extern crate failure;
#[macro_use]
extern crate log;
#[macro_use]
extern crate rocket;
#[macro_use]
extern crate serde_derive;

use crate::models::request_info::RequestInfo;
use crate::services::config_service::Config;
use rocket::response::NamedFile;
use rocket_cache_response::CacheResponse;
use serde_yaml::{from_reader, to_string, Value};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process;

mod apps;
mod commands;
mod models;
mod services;
mod webhooks;

#[get("/")]
fn index() -> CacheResponse<Option<NamedFile>> {
    CacheResponse::Public {
        responder: NamedFile::open(Path::new("frontend/index.html")).ok(),
        max_age: 60 * 60, // cached for seconds
        must_revalidate: false,
    }
}

#[get("/<path..>")]
fn files(path: PathBuf) -> CacheResponse<Option<NamedFile>> {
    CacheResponse::Public {
        responder: NamedFile::open(Path::new("frontend/").join(path)).ok(),
        max_age: 60 * 60, // cached for seconds
        must_revalidate: false,
    }
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
    v["servers"][0]["url"] = Value::String(String::from(url.to_string()));

    Some(to_string(&v).unwrap())
}

fn main() {
    if cfg!(not(debug_assertions)) {
        env_logger::init();
    }

    let config = match Config::load() {
        Ok(config) => config,
        Err(e) => {
            error!("Cannot load config: {}", e);
            process::exit(0x0100);
        }
    };

    rocket::ignite()
        .manage(config)
        .mount("/", routes![index])
        .mount("/openapi.yaml", routes![openapi])
        .mount("/", routes![files])
        .mount("/api", routes![apps::apps])
        .mount("/api", routes![apps::tickets])
        .mount("/api", routes![apps::create_app])
        .mount("/api", routes![apps::delete_app])
        .mount("/api", routes![webhooks::webhooks])
        .launch();
}
