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

use crate::commands::delete_app_command::{DeleteAppCommand, DeleteAppError};
use crate::models::service::Service;
use crate::models::web_hook_info::WebHookInfo;
use rocket_contrib::json::Json;

#[post("/webhooks", format = "application/json", data = "<web_hook_info>")]
pub fn webhooks(
    web_hook_info: WebHookInfo,
    delete_app_command: DeleteAppCommand,
) -> Result<Json<Vec<Service>>, DeleteAppError> {
    info!(
        "Deleting app {:?} through web hook {:?} with event {:?}",
        web_hook_info.get_app_name(),
        web_hook_info.get_title(),
        web_hook_info.get_event_key()
    );

    let services = delete_app_command.delete_app(&web_hook_info.get_app_name())?;
    Ok(Json(services))
}
