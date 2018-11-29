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
use std::collections::HashMap;

use multimap::MultiMap;
use rocket::http::RawStr;
use rocket_contrib::json::Json;

use commands::create_app_command::{CreateOrUpdateAppCommand, CreateOrUpdateError};
use commands::delete_app_command::{DeleteAppCommand, DeleteAppError};
use commands::list_apps_command::{ListAppsCommand, ListAppsError};
use commands::list_tickets_command::{ListTicketsCommand, ListTicketsError};
use models::service::Service;
use models::ticket_info::TicketInfo;

#[get("/apps", format = "application/json")]
pub fn apps(
    list_apps_command: ListAppsCommand,
) -> Result<Json<MultiMap<String, Service>>, ListAppsError> {
    let apps = list_apps_command.list_apps()?;
    Ok(Json(apps))
}

#[get("/apps/tickets", format = "application/json")]
pub fn tickets(
    list_tickets_command: ListTicketsCommand,
) -> Result<Json<HashMap<String, TicketInfo>>, ListTicketsError> {
    let apps = list_tickets_command.list_ticket_infos()?;
    Ok(Json(apps))
}

#[delete("/apps/<app_name>", format = "application/json")]
pub fn delete_app(
    app_name: &RawStr,
    delete_app_command: DeleteAppCommand,
) -> Result<Json<Vec<Service>>, DeleteAppError> {
    let services = delete_app_command.delete_app(&app_name.to_string())?;
    Ok(Json(services))
}

#[post(
    "/apps/<app_name>",
    format = "application/json",
    data = "<app_command>"
)]
pub fn create_app(
    app_name: &RawStr,
    app_command: CreateOrUpdateAppCommand,
) -> Result<Json<Vec<Service>>, CreateOrUpdateError> {
    let services = app_command.create_or_update_app(&app_name.to_string())?;
    Ok(Json(services))
}
