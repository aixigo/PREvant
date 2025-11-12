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

use crate::config::{Config, JiraConfig};
use crate::http_result::{HttpApiError, HttpResult};
use crate::models::ticket_info::TicketInfo;
use crate::models::{App, AppName};
use evmap::{ReadHandleFactory, WriteHandle};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use http_api_problem::{HttpApiProblem, StatusCode};
use jira_query::{JiraInstance, JiraQueryError};
use log::debug;
use rocket::fairing::{Fairing, Info};
use rocket::serde::json::Json;
use rocket::{Build, Orbit, Rocket, State};
use std::collections::HashMap;
use std::convert::From;
use std::sync::Mutex;
use tokio::sync::watch::Receiver;
use tokio_stream::wrappers::WatchStream;

pub fn ticket_routes() -> Vec<rocket::Route> {
    rocket::routes![tickets_route]
}

#[rocket::get("/apps/tickets", format = "application/json")]
fn tickets_route(cache: &State<TicketsCache>) -> HttpResult<Json<SerializableTickets>> {
    match cache.serializable_tickets() {
        Some(tickets) => Ok(Json(tickets)),
        None => Err(ListTicketsError::MissingIssueTrackingConfiguration.into()),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ListTicketsError {
    #[error("No issue tracking configuration")]
    MissingIssueTrackingConfiguration,
}

impl From<ListTicketsError> for HttpApiError {
    fn from(error: ListTicketsError) -> Self {
        let status = match error {
            ListTicketsError::MissingIssueTrackingConfiguration => StatusCode::NO_CONTENT,
        };

        HttpApiProblem::with_title_and_type(status)
            .detail(format!("{error}"))
            .into()
    }
}

pub struct TicketsCaching {
    reader_factory: ReadHandleFactory<AppName, Box<TicketInfo>>,
    writer: Mutex<Option<WriteHandle<AppName, Box<TicketInfo>>>>,
}

impl TicketsCaching {
    pub fn fairing() -> Self {
        let (reader, writer) = evmap::new();
        Self {
            reader_factory: reader.factory(),
            writer: Mutex::new(Some(writer)),
        }
    }
}

#[rocket::async_trait]
impl Fairing for TicketsCaching {
    fn info(&self) -> Info {
        Info {
            name: "ticket-cache",
            kind: rocket::fairing::Kind::Ignite | rocket::fairing::Kind::Liftoff,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<Build>) -> rocket::fairing::Result {
        let Some(config) = rocket.state::<Config>() else {
            log::error!("No configuration object available");
            return Err(rocket);
        };

        match config.jira_config() {
            Some(_jira_config) => Ok(rocket.manage(TicketsCache {
                reader_factory: Some(self.reader_factory.clone()),
            })),
            None => Ok(rocket.manage(TicketsCache {
                reader_factory: None,
            })),
        }
    }

    async fn on_liftoff(&self, rocket: &Rocket<Orbit>) {
        let Some(config) = rocket.state::<Config>() else {
            todo!()
        };

        if let Some(jira_config) = config.jira_config() {
            let app_updates = rocket
                .state::<Receiver<HashMap<AppName, App>>>()
                .expect("App update should be available");

            let writer = {
                let mut writer = self.writer.lock().unwrap();
                writer.take().unwrap()
            };
            let crawler = TicketsCrawler {
                jira_config,
                app_updates: app_updates.clone(),
                writer,
            };

            rocket::tokio::spawn(async move { crawler.spawn_loop().await });
        }
    }
}

struct TicketsCache {
    reader_factory: Option<ReadHandleFactory<AppName, Box<TicketInfo>>>,
}

struct SerializableTickets(ReadHandleFactory<AppName, Box<TicketInfo>>);

impl serde::ser::Serialize for SerializableTickets {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let handle = self.0.handle();

        let mut map = serializer.serialize_map(None)?;
        if let Some(read) = handle.read() {
            for (app_name, ticket) in read.iter() {
                map.serialize_entry(app_name, ticket.get_one().unwrap())?;
            }
        }
        map.end()
    }
}

impl TicketsCache {
    fn serializable_tickets(&self) -> Option<SerializableTickets> {
        self
            .reader_factory
            .as_ref()
            .map(|reader_factory| SerializableTickets(reader_factory.clone()))
    }
}

struct TicketsCrawler {
    jira_config: JiraConfig,
    app_updates: Receiver<HashMap<AppName, App>>,
    writer: WriteHandle<AppName, Box<TicketInfo>>,
}

impl TicketsCrawler {
    async fn spawn_loop(mut self) {
        let mut app_updates = WatchStream::from_changes(self.app_updates);
        let mut app_names = Vec::<AppName>::new();
        loop {
            tokio::select! {
                Some(changed) = app_updates.next() => {
                    log::debug!("Got new app names");
                    app_names = changed.into_keys().collect::<Vec<_>>();
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(120)) => {
                    log::debug!("Regular tickets check.");
                }
            }

            if app_names.is_empty() {
                continue;
            }

            let jira = JiraInstance::at(self.jira_config.host().clone())
                .unwrap()
                .authenticate(match self.jira_config.auth() {
                    crate::config::JiraAuth::Basic { user, password } => jira_query::Auth::Basic {
                        user: user.clone(),
                        password: password.0.unsecure().to_string(),
                    },
                    crate::config::JiraAuth::ApiKey { api_key } => {
                        jira_query::Auth::ApiKey(api_key.0.unsecure().to_string())
                    }
                });

            self.writer.purge();

            let app_names_clone = app_names.clone();
            let mut futures = app_names_clone
                .iter()
                .map(|app_name| async { (app_name.clone(), jira.issue(app_name).await) })
                .collect::<FuturesUnordered<_>>();

            while let Some((app_name, jira_result)) = futures.next().await {
                match jira_result {
                    Ok(issue) => {
                        debug!("Found issue {}", issue.key);
                        self.writer
                            .insert(app_name, Box::new(TicketInfo::from(issue)));
                    }
                    Err(JiraQueryError::MissingIssues(issues)) => {
                        debug!("Cannot query issue information for {issues:?}");
                    }
                    Err(JiraQueryError::Request(err)) if err.is_decode() => {
                        debug!("Cannot deserialize issue, assuming it cannot be decoded due to issue not being found: {err}");
                    }
                    Err(err) => {
                        log::error!("Cannot fetch ticketing information: {err}");
                    }
                }
            }

            self.writer.refresh();
            self.writer.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_json_diff::assert_json_eq;
    use rocket::{http::ContentType, local::asynchronous::Client};
    use std::str::FromStr;
    use url::Url;

    #[tokio::test]
    async fn respond_with_filled_ticket_cache() {
        let (reader, mut writer) = evmap::new();

        writer.insert(
            AppName::master(),
            Box::new(TicketInfo {
                link: Url::from_str("https://tickets.example.com").unwrap(),
                summary: String::from("summary"),
                status: String::from("status"),
            }),
        );
        writer.refresh();
        writer.flush();

        let cache = TicketsCache {
            reader_factory: Some(reader.factory()),
        };
        let rocket = rocket::build()
            .mount("/", rocket::routes![tickets_route])
            .manage(cache);

        let client = Client::tracked(rocket).await.expect("valid rocket config");
        let response = client
            .get("/apps/tickets")
            .header(ContentType::JSON)
            .dispatch()
            .await;

        let body_str = response.into_string().await.expect("valid response body");
        let actual: serde_json::Value = serde_json::from_str(&body_str).unwrap();
        assert_json_eq!(
            actual,
            serde_json::json!({
                "master": {
                    "link": "https://tickets.example.com/",
                    "summary": "summary",
                    "status": "status",
                }
            })
        )
    }

    #[tokio::test]
    async fn respond_with_empty_ticket_cache() {
        let (reader, _writer) = evmap::new();

        let cache = TicketsCache {
            reader_factory: Some(reader.factory()),
        };
        let rocket = rocket::build()
            .mount("/", rocket::routes![tickets_route])
            .manage(cache);

        let client = Client::tracked(rocket).await.expect("valid rocket config");
        let response = client
            .get("/apps/tickets")
            .header(ContentType::JSON)
            .dispatch()
            .await;

        let body_str = response.into_string().await.expect("valid response body");
        let actual: serde_json::Value = serde_json::from_str(&body_str).unwrap();
        assert_json_eq!(actual, serde_json::json!({}))
    }

    #[tokio::test]
    async fn respond_with_no_ticket_information() {
        let cache = TicketsCache {
            reader_factory: None,
        };

        let rocket = rocket::build()
            .mount("/", rocket::routes![tickets_route])
            .manage(cache);

        let client = Client::tracked(rocket).await.expect("valid rocket config");
        let response = client
            .get("/apps/tickets")
            .header(ContentType::JSON)
            .dispatch()
            .await;

        assert_eq!(rocket::http::Status::NoContent, response.status())
    }
}
