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
use crate::apps::{delete_app_sync, AppV1};
use crate::auth::UserValidatedByAccessMode;
use crate::http_result::HttpResult;
use crate::models::web_hook_info::WebHookInfo;
use crate::models::AppName;
use http_api_problem::HttpApiProblem;
use log::info;
use rocket::serde::json::Json;
use rocket::State;
use std::str::FromStr;
use std::sync::Arc;

#[rocket::post("/webhooks", format = "application/json", data = "<web_hook_info>")]
pub async fn webhooks(
    apps: &State<Arc<Apps>>,
    web_hook_info: WebHookInfo,
    user: Result<UserValidatedByAccessMode, HttpApiProblem>,
) -> HttpResult<Json<AppV1>> {
    info!(
        "Deleting app {:?} through web hook {:?} with event {:?}",
        web_hook_info.get_app_name(),
        web_hook_info.get_title(),
        web_hook_info.get_event_key()
    );

    let app_name = AppName::from_str(&web_hook_info.get_app_name());
    delete_app_sync(app_name, apps, user).await
}
