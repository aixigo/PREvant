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
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use http_api_problem::{HttpApiProblem, StatusCode};
use jira_query::{JiraInstance, JiraQueryError};
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
            if services.is_empty() {
                return Ok(Json(tickets));
            }

            let jira = JiraInstance::at(jira_config.host().clone())
                .unwrap()
                .authenticate(match jira_config.auth() {
                    crate::config::JiraAuth::Basic { user, password } => jira_query::Auth::Basic {
                        user: user.clone(),
                        password: password.unsecure().to_string(),
                    },
                    crate::config::JiraAuth::ApiKey { api_key } => {
                        jira_query::Auth::ApiKey(api_key.unsecure().to_string())
                    }
                });

            let mut futures = services
                .keys()
                .map(|app_name| jira.issue(app_name))
                .collect::<FuturesUnordered<_>>();

            while let Some(r) = futures.next().await {
                match r {
                    Ok(issue) => {
                        tickets.insert(issue.key.clone(), TicketInfo::from(issue));
                    }
                    Err(JiraQueryError::MissingIssues(issues)) => {
                        debug!("Cannot query issue information for {issues:?}");
                    }
                    Err(JiraQueryError::Request(err)) if err.is_decode() => {
                        debug!("Cannot deserialize issue, assuming it cannot be decoded due to issue not being found: {err}");
                    }
                    Err(err) => {
                        return Err(ListTicketsError::UnexpectedError {
                            err: anyhow::Error::new(err),
                        }
                        .into());
                    }
                }
            }
        }
    };

    Ok(Json(tickets))
}

#[derive(Debug, thiserror::Error)]
pub enum ListTicketsError {
    #[error("No issue tracking configuration")]
    MissingIssueTrackingConfiguration,
    #[error("Unexpected issue tracking system error: {err}")]
    UnexpectedError { err: anyhow::Error },
}

impl From<ListTicketsError> for HttpApiError {
    fn from(error: ListTicketsError) -> Self {
        let status = match error {
            ListTicketsError::MissingIssueTrackingConfiguration => StatusCode::NO_CONTENT,
            ListTicketsError::UnexpectedError { err: _ } => StatusCode::INTERNAL_SERVER_ERROR,
        };

        HttpApiProblem::with_title_and_type(status)
            .detail(format!("{}", error))
            .into()
    }
}
