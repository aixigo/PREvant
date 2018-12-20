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
use std::io::Read;

use rocket::data::{self, FromDataSimple};
use rocket::http::{ContentType, Status};
use rocket::request::{FromRequest, Request};
use rocket::response::{self, Responder, Response};
use rocket::Data;
use rocket::Outcome::{Failure, Success};
use rocket_contrib::json;
use serde_json::from_str;

use models::request_info::RequestInfo;
use models::service::{Service, ServiceConfig, ServiceError};

use services::apps_service::{AppsService, AppsServiceError};

pub struct CreateOrUpdateAppCommand {
    data: Vec<ServiceConfig>,
    request_info: RequestInfo,
}

impl FromDataSimple for CreateOrUpdateAppCommand {
    type Error = String;

    fn from_data(request: &Request, data: Data) -> data::Outcome<CreateOrUpdateAppCommand, String> {
        let mut body = String::new();
        if let Err(e) = data.open().read_to_string(&mut body) {
            return Failure((Status::InternalServerError, format!("{:?}", e)));
        }

        let data = match from_str::<Vec<ServiceConfig>>(&body) {
            Ok(v) => v,
            Err(err) => {
                return Failure((
                    Status::BadRequest,
                    format!("Cannot read body as JSON: {:?}", err),
                ));
            }
        };

        let request_info = match RequestInfo::from_request(request) {
            Success(r) => r,
            err => return Failure((Status::BadRequest, format!("{:?}", err))),
        };

        Success(CreateOrUpdateAppCommand { data, request_info })
    }
}

impl CreateOrUpdateAppCommand {
    /// Executes the command to create or update an app.
    ///
    /// First, the command tries to pull a docker image given by the request body, e.g.
    /// `{ registry: "registry01.dev.aixigo.de", serviceName: "aam-service", env: [] }`
    /// and the given app name, e.g. `master` (the full docker image will be `registry01.dev.aixigo.de/aam/aam-service`).
    ///
    /// Then, the command checks if there is already an instance running and it will stop it
    /// and then it starts a new instance.
    pub fn create_or_update_app(
        &self,
        app_name: &String,
    ) -> Result<Vec<Service>, CreateOrUpdateError> {
        let apps_service = AppsService::new()?;
        let mut services = apps_service.create_or_update(app_name, &self.data)?;

        for service in services.iter_mut() {
            service.set_base_url(self.request_info.get_base_url());
        }

        Ok(services)
    }
}

#[derive(Debug)]
pub enum CreateOrUpdateError {
    BadServiceConfiguration(ServiceError),
    Internal(String),
}

impl From<AppsServiceError> for CreateOrUpdateError {
    fn from(err: AppsServiceError) -> Self {
        match err {
            AppsServiceError::InfrastructureError(err) => {
                CreateOrUpdateError::Internal(format!("{:?}", err))
            }
            AppsServiceError::InvalidServiceModel(service_error) => {
                CreateOrUpdateError::BadServiceConfiguration(service_error)
            }
            msg => CreateOrUpdateError::Internal(format!("{:?}", msg)),
        }
    }
}

impl Error for CreateOrUpdateError {
    fn description(&self) -> &str {
        "Create or Update Error"
    }
}

impl Display for CreateOrUpdateError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            CreateOrUpdateError::BadServiceConfiguration(service_error) => f.write_str(&format!(
                "Invalid configuration for service {:?}",
                service_error
            )),
            CreateOrUpdateError::Internal(err) => f.write_str(&format!("{:?}", err)),
        }
    }
}

impl<'r> Responder<'r> for CreateOrUpdateError {
    fn respond_to(self, _: &Request) -> response::Result<'r> {
        match self {
            CreateOrUpdateError::Internal(err) => Response::build()
                .sized_body(Cursor::new(
                    json!({ "error": format!("Internal error: {:?}", err) }).to_string(),
                ))
                .header(ContentType::JSON)
                .status(Status::InternalServerError)
                .ok(),
            CreateOrUpdateError::BadServiceConfiguration(err) => Response::build()
                .sized_body(Cursor::new(
                    json!({ "error": format!("{:?}", err) }).to_string(),
                ))
                .header(ContentType::JSON)
                .status(Status::BadRequest)
                .ok(),
        }
    }
}
