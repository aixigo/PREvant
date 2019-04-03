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
use rocket::request::{self, FromRequest, Request};
use rocket::Outcome;
use url::Url;

pub struct RequestInfo {
    base_url: Url,
}

impl RequestInfo {
    #[cfg(test)]
    pub fn new(base_url: Url) -> Self {
        RequestInfo { base_url }
    }

    /// Returns the value for the `host` value of the
    /// [Forwarded](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded) header.
    pub fn host(&self) -> String {
        match self.base_url.scheme() {
            "http" => match self.base_url.port() {
                None | Some(80) => String::from(self.base_url.host_str().unwrap()),
                Some(port) => format!("{}:{}", self.base_url.host_str().unwrap(), port),
            },
            "https" => match self.base_url.port() {
                None | Some(443) => String::from(self.base_url.host_str().unwrap()),
                Some(port) => format!("{}:{}", self.base_url.host_str().unwrap(), port),
            },
            _ => String::from(self.base_url.host_str().unwrap()),
        }
    }

    /// Returns the value for the `proto` value of the
    /// [Forwarded](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded) header.
    pub fn scheme(&self) -> &str {
        self.base_url.scheme()
    }

    pub fn get_base_url(&self) -> &Url {
        &self.base_url
    }
}

impl<'a, 'r> FromRequest<'a, 'r> for RequestInfo {
    type Error = ();

    fn from_request(request: &'a Request<'r>) -> request::Outcome<RequestInfo, ()> {
        let hosts: Vec<_> = request.headers().get("host").collect();

        if hosts.len() != 1 {
            return Outcome::Failure((Status::BadRequest, ()));
        }

        let url_string = "http://".to_owned() + &hosts[0];
        match Url::parse(&url_string) {
            Ok(url) => return Outcome::Success(RequestInfo { base_url: url }),
            Err(_) => return Outcome::Failure((Status::BadRequest, ())),
        }
    }
}
