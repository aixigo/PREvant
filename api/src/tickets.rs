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

use crate::apps::Apps;
use crate::config::Config;
use crate::http_result::{HttpApiError, HttpResult};
use crate::models::ticket_info::TicketInfo;
use goji::Error as GojiError;
use goji::{Credentials, Jira, SearchOptions};
use http_api_problem::{HttpApiProblem, StatusCode};
use rocket::serde::json::Json;
use rocket::State;
use std::collections::HashMap;
use std::convert::From;
use std::sync::Arc;

/// Analyzes running containers and returns a map of `review-app-name` with the
/// corresponding `TicketInfo`.
#[get("/apps/tickets", format = "application/json")]
pub async fn tickets(
    config_state: &State<Config>,
    apps_service: &State<Arc<Apps>>,
) -> HttpResult<Json<HashMap<String, TicketInfo>>> {
    let mut tickets: HashMap<String, TicketInfo> = HashMap::new();

    match config_state.jira_config() {
        None => {
            return Err(ListTicketsError::MissingIssueTrackingConfiguration.into());
        }
        Some(jira_config) => {
            let services = apps_service.get_apps().await?;

            let pw = String::from(jira_config.password().unsecure());
            let jira = match Jira::new(
                jira_config.host().clone(),
                Credentials::Basic(jira_config.user().clone(), pw),
            ) {
                Ok(jira) => jira,
                Err(e) => return Err(ListTicketsError::from(e).into()),
            };

            let issue_keys = services
                .keys()
                .map(|s| format!("{:?}", s))
                .collect::<Vec<String>>()
                .join(", ");

            debug!("Search for issues: {}", issue_keys);

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
                    err => {
                        let e = ListTicketsError::from(err);
                        error!("Cannot retrieve ticket information: {}", e);
                        return Err(e.into());
                    }
                },
            }
        }
    }

    Ok(Json(tickets))
}

#[derive(Debug, Fail)]
pub enum ListTicketsError {
    #[fail(display = "No issue tracking configuration")]
    MissingIssueTrackingConfiguration,
    #[fail(
        display = "Unexpected issue tracking system error: {}",
        internal_message
    )]
    UnexpectedError { internal_message: String },
}

impl From<ListTicketsError> for HttpApiError {
    fn from(error: ListTicketsError) -> Self {
        let status = match error {
            ListTicketsError::MissingIssueTrackingConfiguration => StatusCode::NO_CONTENT,
            ListTicketsError::UnexpectedError {
                internal_message: _,
            } => StatusCode::INTERNAL_SERVER_ERROR,
        };

        HttpApiProblem::with_title_and_type(status)
            .detail(format!("{}", error))
            .into()
    }
}

impl From<GojiError> for ListTicketsError {
    fn from(err: GojiError) -> Self {
        ListTicketsError::UnexpectedError {
            internal_message: format!("{:?}", err),
        }
    }
}
