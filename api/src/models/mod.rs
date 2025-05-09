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

pub use app::{
    App, AppWithHostMeta, ContainerType, Owner, Service, ServiceError, ServiceStatus,
    ServiceWithHostMeta, State,
};
pub use app_name::{AppName, AppNameError};
pub use app_status_change_id::{AppStatusChangeId, AppStatusChangeIdError};
pub use image::Image;
pub use logs_chunks::LogChunk;
pub use request_info::RequestInfo;
pub use service_config::{Environment, EnvironmentVariable, ServiceConfig};
pub use web_host_meta::WebHostMeta;

#[cfg_attr(test, macro_use)]
mod app;
mod app_name;
mod app_status_change_id;
mod image;
mod logs_chunks;
pub mod request_info;
mod service_config;
pub mod ticket_info;
pub mod user_defined_parameters;
pub mod web_hook_info;
pub mod web_host_meta;
