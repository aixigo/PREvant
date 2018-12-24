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
use regex::Regex;
use rocket::data::{self, FromDataSimple};
use rocket::http::Status;
use rocket::request::Request;
use rocket::Data;
use rocket::Outcome::{Failure, Success};
use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer};
use serde_json::from_str;
use std::io::Read;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebHookInfo {
    event_key: EventKey,
    pull_request: PullRequest,
}

#[derive(Debug)]
pub enum EventKey {
    MergedPullRequest,
    DeclinedPullRequest,
    DeletedPullRequest,
}

impl<'de> Deserialize<'de> for EventKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let event_key = String::deserialize(deserializer)?;

        match event_key.as_str() {
            "pr:merged" => Ok(EventKey::MergedPullRequest),
            "pr:declined" => Ok(EventKey::DeclinedPullRequest),
            "pr:deleted" => Ok(EventKey::DeletedPullRequest),
            _ => Err(DeserializeError::custom(format!(
                "Unsupported event key {:?}",
                event_key
            ))),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    title: String,
    from_ref: Ref,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ref {
    display_id: String,
}

impl WebHookInfo {
    pub fn get_title(&self) -> &String {
        &self.pull_request.title
    }

    pub fn get_app_name(&self) -> String {
        let re: Regex = Regex::new(r"[A-Z]{3,}-\d+").unwrap();

        match re.captures(&self.pull_request.from_ref.display_id) {
            Some(c) => String::from(c.get(0).unwrap().as_str()),
            None => self.pull_request.from_ref.display_id.clone(),
        }
    }

    pub fn get_event_key(&self) -> &EventKey {
        &self.event_key
    }
}

impl FromDataSimple for WebHookInfo {
    type Error = String;

    fn from_data(_request: &Request, data: Data) -> data::Outcome<WebHookInfo, String> {
        let mut body = String::new();
        if let Err(e) = data.open().read_to_string(&mut body) {
            return Failure((Status::InternalServerError, format!("{:?}", e)));
        }

        let data = match from_str::<WebHookInfo>(&body) {
            Ok(v) => v,
            Err(err) => {
                return Failure((
                    Status::BadRequest,
                    format!("Cannot read body as JSON: {:?}", err),
                ));
            }
        };

        Success(data)
    }
}
