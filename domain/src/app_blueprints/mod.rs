//! Provides [value objects](https://en.wikipedia.org/wiki/Value_object) that allow to model from
//! the user perspective a desired application state.

use serde::{Deserialize, Serialize};
pub use service::{Environment, EnvironmentVariable, ServiceConfig};
pub use user_defined_parameters::{UserDefinedParameters, UserDefinedParametersError};

#[macro_use]
mod service;
mod user_defined_parameters;

/// Input value to describe that a user wants to change the runtime-status of a service.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DesiredServiceStatus {
    Running,
    Paused,
}
