//! Based on the module [`app_deployment`](`crate::app_deployment`), that models what has to be
//! deployed onto the infrastructure, this module provides domain objects that describe a running
//! application.

pub use app::{App, AppWithHostMeta, Service, ServiceStatus, ServiceWithHostMeta};
pub use status::{AppStatus, ContainerType, ContainerTypeParseError};
pub use web_host_meta::WebHostMeta;

mod app;
mod status;
mod web_host_meta;
