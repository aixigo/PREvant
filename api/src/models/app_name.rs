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

use http_api_problem::{HttpApiProblem, StatusCode};
use regex::Regex;
use rocket::http::RawStr;
use rocket::request::FromParam;
use std::collections::HashSet;
use std::ops::Deref;
use std::str::{FromStr, Utf8Error};

#[derive(Debug)]
pub struct AppName(String);

impl Deref for AppName {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromStr for AppName {
    type Err = AppNameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        lazy_static! {
            static ref INVALID_CHARS_REGEX: Regex = Regex::new("(\\s|/)").unwrap();
        }

        match INVALID_CHARS_REGEX.captures(name) {
            None => Ok(AppName(name.to_string())),
            Some(captures) => {
                let invalid_chars = captures
                    .iter()
                    .filter_map(|c| c)
                    .map(|c| c.as_str())
                    .collect::<HashSet<&str>>()
                    .into_iter()
                    .collect::<Vec<&str>>()
                    .join("");

                return Err(AppNameError::InvalidChars { invalid_chars });
            }
        }
    }
}

impl<'r> FromParam<'r> for AppName {
    type Error = AppNameError;

    fn from_param(param: &'r RawStr) -> Result<Self, Self::Error> {
        AppName::from_str(&param.url_decode()?)
    }
}

#[derive(Debug, Fail)]
pub enum AppNameError {
    #[fail(
        display = "Invalid characters in app name: “{}” are invalid.",
        invalid_chars
    )]
    InvalidChars { invalid_chars: String },
    #[fail(display = "Invalid url encoded parameter: {}", err)]
    InvalidUrlDecodedParam { err: String },
}

impl From<Utf8Error> for AppNameError {
    fn from(err: Utf8Error) -> Self {
        AppNameError::InvalidUrlDecodedParam {
            err: format!("{}", err),
        }
    }
}

impl From<AppNameError> for HttpApiProblem {
    fn from(err: AppNameError) -> Self {
        HttpApiProblem::with_title_from_status(StatusCode::BAD_REQUEST)
            .set_detail(format!("{}", err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_app_name_from_str() {
        let app_name = AppName::from_str("master").unwrap();

        assert_eq!(app_name.0, "master");
    }

    #[test]
    fn should_create_app_name_from_utf_str() {
        let app_name = AppName::from_str("Üß¥$Ω").unwrap();

        assert_eq!(app_name.0, "Üß¥$Ω");
    }

    #[test]
    fn should_not_create_app_name_app_name_contains_whitespaces() {
        let app_name = AppName::from_str(" master\n ");

        assert!(app_name.is_err());
    }

    #[test]
    fn should_not_create_app_name_app_name_contains_slashes() {
        let app_name = AppName::from_str("feature/xxx");

        assert!(app_name.is_err());
    }
}
