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

use crate::models::Environment;
pub use docker::DockerInfrastructure as Docker;
#[cfg(test)]
pub use dummy_infrastructure::DummyInfrastructure as Dummy;
pub use infrastructure::{DeploymentStrategy, Infrastructure};
pub use kubernetes::KubernetesInfrastructure as Kubernetes;
use serde_json::{map::Map, Value};

mod docker;
#[cfg(test)]
mod dummy_infrastructure;
mod infrastructure;
mod kubernetes;

static APP_NAME_LABEL: &str = "com.aixigo.preview.servant.app-name";
static SERVICE_NAME_LABEL: &str = "com.aixigo.preview.servant.service-name";
static CONTAINER_TYPE_LABEL: &str = "com.aixigo.preview.servant.container-type";
static REPLICATED_ENV_LABEL: &str = "com.aixigo.preview.servant.replicated-env";
static IMAGE_LABEL: &str = "com.aixigo.preview.servant.image";
static STATUS_ID: &str = "com.aixigo.preview.servant.status-id";

/// This function converts the environment variables and adds all variables, that
/// must be replicated, into a JSON object. This function should be used by implementations
/// to serialize the environment variable so that it can be deserialized when service configurations
/// will be cloned from a running service.
fn replicated_environment_variable_to_json(env: &Environment) -> Option<Value> {
    let replicated_env = env
        .iter()
        .filter(|ev| ev.replicate())
        .map(|ev| {
            (
                ev.key(),
                serde_json::json!({
                    "value": ev.original().value().unsecure(),
                    "templated": ev.templated(),
                    "replicate": true
                }),
            )
        })
        .fold(Map::<String, Value>::new(), |mut acc, (key, value)| {
            acc.insert(key.clone(), value);
            acc
        });

    if !replicated_env.is_empty() {
        Some(Value::Object(replicated_env))
    } else {
        None
    }
}
