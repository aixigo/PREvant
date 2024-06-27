/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2021 aixigo AG
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

use http_api_problem::HttpApiProblem;
use rocket::http::{hyper::header::CONTENT_TYPE, Header, Status};
use rocket::request::Request;
use rocket::response::{self, Responder, Response};
use std::convert::From;
use std::io::Cursor;

pub type HttpResult<T> = Result<T, HttpApiError>;

#[derive(Debug)]
pub struct HttpApiError(HttpApiProblem);

impl From<HttpApiProblem> for HttpApiError {
    fn from(problem: HttpApiProblem) -> Self {
        Self(problem)
    }
}

impl<'r> Responder<'r, 'static> for HttpApiError {
    fn respond_to(self, request: &'r Request<'_>) -> response::Result<'static> {
        if self.0.status == Some(http_api_problem::StatusCode::NO_CONTENT) {
            return rocket::response::status::NoContent.respond_to(request);
        }

        let paylaod = self.0.json_bytes();
        Response::build()
            .header(Header::new(
                CONTENT_TYPE.as_str(),
                "application/problem+json",
            ))
            .status(
                self.0
                    .status
                    .and_then(|status| Status::from_code(status.as_u16()))
                    .unwrap_or_default(),
            )
            .sized_body(paylaod.len(), Cursor::new(paylaod))
            .ok()
    }
}
