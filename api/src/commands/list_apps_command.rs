/*-
 * ========================LICENSE_START=================================
 * PREvant
 * %%
 * Copyright (C) 2018 aixigo AG
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
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io::Cursor;

use multimap::MultiMap;
use rocket::http::{ContentType, Status};
use rocket::request::{self, FromRequest, Request};
use rocket::response::{self, Responder, Response};
use rocket::Outcome::{Failure, Success};
use rocket_contrib::json;

use models::request_info::RequestInfo;
use models::service::Service;
use services::apps_service::{AppsService, AppsServiceError};

pub struct ListAppsCommand {
    request_info: RequestInfo,
}

impl<'a, 'r> FromRequest<'a, 'r> for ListAppsCommand {
    type Error = String;

    fn from_request(request: &Request) -> request::Outcome<ListAppsCommand, String> {
        let request_info = match RequestInfo::from_request(request) {
            Success(r) => r,
            err => return Failure((Status::BadRequest, format!("{:?}", err))),
        };

        Success(ListAppsCommand { request_info })
    }
}

impl ListAppsCommand {
    /// Analyzes running containers and returns a map of `review-app-name` with the
    /// corresponding list of `Service`s.
    pub fn list_apps(&self) -> Result<MultiMap<String, Service>, ListAppsError> {
        let apps_service = AppsService::new()?;
        let mut apps = apps_service.get_apps()?;

        for (_, services) in apps.iter_all_mut() {
            for service in services.iter_mut() {
                service.set_base_url(self.request_info.get_base_url());
            }
        }

        Ok(apps)
    }
}

#[derive(Debug)]
pub enum ListAppsError {
    Internal(String),
}

impl From<AppsServiceError> for ListAppsError {
    fn from(err: AppsServiceError) -> Self {
        ListAppsError::Internal(format!("{:?}", err))
    }
}

impl Error for ListAppsError {
    fn description(&self) -> &str {
        "List Apps Error"
    }
}

impl Display for ListAppsError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            ListAppsError::Internal(err) => f.write_str(&format!("Internal error: {:?}", err)),
        }
    }
}

impl<'r> Responder<'r> for ListAppsError {
    fn respond_to(self, _: &Request) -> response::Result<'r> {
        match self {
            ListAppsError::Internal(error) => Response::build()
                .sized_body(Cursor::new(json!({ "error": error }).to_string()))
                .header(ContentType::JSON)
                .status(Status::InternalServerError)
                .ok(),
        }
    }
}
