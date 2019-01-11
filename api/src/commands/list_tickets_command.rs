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
use crate::models::ticket_info::TicketInfo;
use crate::services::apps_service::{AppsService, AppsServiceError};
use crate::services::config_service::{Config, ConfigError};
use goji::Error as GojiError;
use goji::{Credentials, Jira, SearchOptions};
use rocket::request::{self, FromRequest, Request};
use rocket::Outcome::Success;
use std::collections::HashMap;

pub struct ListTicketsCommand {}

impl<'a, 'r> FromRequest<'a, 'r> for ListTicketsCommand {
    type Error = String;

    fn from_request(_request: &Request) -> request::Outcome<ListTicketsCommand, String> {
        Success(ListTicketsCommand {})
    }
}

impl ListTicketsCommand {
    /// Analyzes running containers and returns a map of `review-app-name` with the
    /// corresponding `TicketInfo`.
    pub fn list_ticket_infos(&self) -> Result<HashMap<String, TicketInfo>, ListTicketsError> {
        let mut tickets: HashMap<String, TicketInfo> = HashMap::new();

        match Config::load()?.get_jira_config() {
            None => {
                return Err(ListTicketsError::MissingIssueTrackingConfiguration(
                    "".to_string(),
                ));
            }
            Some(jira_config) => {
                let apps_service = AppsService::new()?;
                let services = apps_service.get_apps()?;

                let jira = Jira::new(
                    jira_config.get_host(),
                    Credentials::Basic(jira_config.get_user(), jira_config.get_password()),
                )?;

                let issue_keys = services
                    .keys()
                    .map(|s| format!("{:?}", s))
                    .collect::<Vec<String>>()
                    .join(", ");

                let options = SearchOptions::builder().validate(false).build();

                match jira
                    .search()
                    .iter(format!("issuekey in ({})", issue_keys), &options)
                {
                    Ok(issues) => {
                        for issue in issues {
                            tickets.insert(issue.key.clone(), TicketInfo::from(issue));
                        }
                    }
                    Err(err) => match err {
                        GojiError::Fault { code, errors } => {
                            debug!("No issue for {}: {:?} {:?}", issue_keys, code, errors)
                        }
                        err => return Err(ListTicketsError::from(err)),
                    },
                }
            }
        }

        Ok(tickets)
    }
}

#[derive(Debug, Fail, Responder)]
pub enum ListTicketsError {
    #[fail(display = "No issue tracking configuration")]
    #[response(status = 204)]
    MissingIssueTrackingConfiguration(String),
    #[fail(
        display = "Unexpected issue tracking system error: {}",
        internal_message
    )]
    #[response(status = 500)]
    UnexpectedError { internal_message: String },
}

impl From<AppsServiceError> for ListTicketsError {
    fn from(err: AppsServiceError) -> Self {
        ListTicketsError::UnexpectedError {
            internal_message: format!("{:?}", err),
        }
    }
}

impl From<ConfigError> for ListTicketsError {
    fn from(err: ConfigError) -> Self {
        ListTicketsError::UnexpectedError {
            internal_message: format!("{:?}", err),
        }
    }
}

impl From<GojiError> for ListTicketsError {
    fn from(err: GojiError) -> Self {
        ListTicketsError::UnexpectedError {
            internal_message: format!("{:?}", err),
        }
    }
}
