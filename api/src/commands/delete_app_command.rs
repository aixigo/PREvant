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

use crate::models::request_info::RequestInfo;
use crate::models::service::Service;
use crate::services::apps_service::{AppsService, AppsServiceError};
use rocket::http::{ContentType, Status};
use rocket::request::{self, FromRequest, Request};
use rocket::response::{self, Responder, Response};
use rocket::Outcome::{Failure, Success};
use rocket_contrib::json;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io::Cursor;

pub struct DeleteAppCommand {
    request_info: RequestInfo,
}

impl<'a, 'r> FromRequest<'a, 'r> for DeleteAppCommand {
    type Error = String;

    fn from_request(request: &Request) -> request::Outcome<DeleteAppCommand, String> {
        let request_info = match RequestInfo::from_request(request) {
            Success(r) => r,
            err => return Failure((Status::BadRequest, format!("{:?}", err))),
        };

        Success(DeleteAppCommand { request_info })
    }
}

impl DeleteAppCommand {
    pub fn delete_app(&self, app_name: &String) -> Result<Vec<Service>, DeleteAppError> {
        let apps_service = AppsService::new()?;
        let mut services = apps_service.delete_app(app_name)?;

        for service in services.iter_mut() {
            service.set_base_url(self.request_info.get_base_url());
        }

        Ok(services)
    }
}

#[derive(Debug)]
pub enum DeleteAppError {
    Internal(String),
}

impl From<AppsServiceError> for DeleteAppError {
    fn from(err: AppsServiceError) -> Self {
        DeleteAppError::Internal(format!("{:?}", err))
    }
}

impl Error for DeleteAppError {
    fn description(&self) -> &str {
        "Delete App Error"
    }
}

impl Display for DeleteAppError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            DeleteAppError::Internal(err) => f.write_str(&format!("Internal error: {:?}", err)),
        }
    }
}

impl<'r> Responder<'r> for DeleteAppError {
    fn respond_to(self, _: &Request) -> response::Result<'r> {
        match self {
            DeleteAppError::Internal(error) => Response::build()
                .sized_body(Cursor::new(json!({ "error": error }).to_string()))
                .header(ContentType::JSON)
                .status(Status::InternalServerError)
                .ok(),
        }
    }
}
