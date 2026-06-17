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
pub use infrastructure::Kubernetes;

mod deployment_unit;
mod element;
mod infrastructure;
mod payloads;
mod traefik_crds;

#[cfg(test)]
pub async fn create_cluster_and_expose_k8s_config() -> anyhow::Result<(
    testcontainers::ContainerAsync<testcontainers_modules::k3s::K3s>,
    std::path::PathBuf,
    tempfile::TempDir,
)> {
    use testcontainers::{
        ImageExt,
        core::{WaitFor, logs::consumer::logging_consumer::LoggingConsumer},
        runners::AsyncRunner,
    };
    use testcontainers_modules::k3s::K3s;

    let _ = env_logger::builder().is_test(true).try_init();

    let tempdir = tempfile::tempdir()?;
    let config_mount = tempdir.path().to_path_buf();

    let k3s_instance = K3s::default()
                .with_conf_mount(&config_mount)
                .with_privileged(true)
                .with_ready_conditions(vec![WaitFor::message_on_stderr(
                        r#""QuotaMonitor created object count evaluator" resource="ingressroutetcps.traefik.io""#,
                )])
                .with_startup_timeout(std::time::Duration::from_mins(2))
                .with_log_consumer(
                    LoggingConsumer::new()
                    .with_stdout_level(log::Level::Trace)
                    .with_stderr_level(log::Level::Trace),
                )
                .start()
                .await?;

    let mapped_port = k3s_instance
        .get_host_port_ipv4(testcontainers_modules::k3s::KUBE_SECURE_PORT.as_u16())
        .await?;

    let config = k3s_instance.image().read_kube_config()?;

    let config_file = tempdir.path().join("k3s-mapped.yaml");
    let config = config.replace(
        "server: https://127.0.0.1:6443",
        &format!("server: https://127.0.0.1:{mapped_port}"),
    );
    std::fs::write(&config_file, config)?;

    Ok((k3s_instance, config_file, tempdir))
}
