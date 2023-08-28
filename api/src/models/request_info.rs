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

use rocket::http::Status;
use rocket::outcome::Outcome;
use rocket::request::{self, FromRequest, Request};
use url::Url;

#[derive(Clone)]
pub struct RequestInfo {
    base_url: Url,
}

impl RequestInfo {
    pub fn get_base_url(&self) -> &Url {
        &self.base_url
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for RequestInfo {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        let forwarded_host = request.headers().get_one("x-forwarded-host");
        let forwarded_host = match forwarded_host {
            Some(host) => host.to_string(),
            None => match request.headers().get_one("host") {
                Some(host) => host.to_string(),
                None => {
                    return Outcome::Failure((Status::BadRequest, ()));
                }
            },
        };

        let forwarded_proto = request
            .headers()
            .get_one("x-forwarded-proto")
            .map(|proto| proto.to_string())
            .unwrap_or_else(|| String::from("http"));
        let forwarded_port = request
            .headers()
            .get_one("x-forwarded-port")
            .map(|port| format!(":{}", port.to_string()))
            .unwrap_or_else(|| String::from(""));

        let host_url = format!("{}://{}{}", forwarded_proto, forwarded_host, forwarded_port);
        match Url::parse(&host_url) {
            Ok(url) => Outcome::Success(RequestInfo { base_url: url }),
            Err(_) => Outcome::Failure((Status::BadRequest, ())),
        }
    }
}
