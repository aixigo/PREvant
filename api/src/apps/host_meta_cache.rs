/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2021 aixigo AG
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
use crate::infrastructure::HttpForwarder;
use crate::models::service::{
    Service, ServiceStatus, ServiceWithHostMeta, Services, ServicesWithHostMeta,
};
use crate::models::{AppName, RequestInfo, WebHostMeta};
use chrono::{DateTime, Utc};
use evmap::{ReadHandleFactory, WriteHandle};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use http::header::{HOST, USER_AGENT};
use rocket::outcome::Outcome;
use rocket::request::{self, FromRequest, Request};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::watch::{self, Receiver, Sender};
use tokio_stream::wrappers::WatchStream;
use yansi::Paint;

pub struct HostMetaCache {
    reader_factory: ReadHandleFactory<Key, Arc<Value>>,
    update_watch_rx: Receiver<DateTime<Utc>>,
}
pub struct HostMetaCrawler {
    writer: WriteHandle<Key, Arc<Value>>,
    update_watch_tx: Sender<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    app_name: AppName,
    service_id: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Value {
    timestamp: DateTime<Utc>,
    web_host_meta: WebHostMeta,
}

pub fn new() -> (HostMetaCache, HostMetaCrawler) {
    // TODO: eventually we should replace evmap with the watch channel or with another thread safe
    // alternative..
    let (reader, writer) = evmap::new();
    let (update_watch_tx, update_watch_rx) = watch::channel(Utc::now());

    (
        HostMetaCache {
            reader_factory: reader.factory(),
            update_watch_rx,
        },
        HostMetaCrawler {
            writer,
            update_watch_tx,
        },
    )
}

impl HostMetaCache {
    pub fn update_meta_data(
        &self,
        services: HashMap<AppName, Services>,
        request_info: &RequestInfo,
    ) -> HashMap<AppName, ServicesWithHostMeta> {
        let mut assigned_apps = HashMap::new();

        let reader = self.reader_factory.handle();

        for (app_name, services) in services.into_iter() {
            let mut services_with_host_meta = Vec::with_capacity(services.len());

            for service in services.into_iter() {
                let service_id = service.id.clone();
                let key = Key {
                    app_name: app_name.clone(),
                    service_id,
                };

                let web_host_meta = match reader.get_one(&key) {
                    Some(value) => value
                        .web_host_meta
                        .with_base_url(request_info.get_base_url()),
                    None => WebHostMeta::empty(),
                };

                services_with_host_meta.push(ServiceWithHostMeta::from_service_and_web_host_meta(
                    service,
                    web_host_meta,
                    request_info.get_base_url().clone(),
                    &app_name,
                ));
            }

            assigned_apps.insert(
                app_name,
                ServicesWithHostMeta::from(services_with_host_meta),
            );
        }

        assigned_apps
    }

    pub fn cache_updates(&self) -> WatchStream<DateTime<Utc>> {
        WatchStream::from_changes(self.update_watch_rx.clone())
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for HostMetaCache {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        match request.rocket().state::<HostMetaCache>() {
            Some(cache) => Outcome::Success(Self {
                reader_factory: cache.reader_factory.clone(),
                update_watch_rx: cache.update_watch_rx.clone(),
            }),
            None => todo!(),
        }
    }
}

impl HostMetaCrawler {
    pub fn spawn(mut self, apps: Arc<Apps>, apps_updates: Receiver<HashMap<AppName, Services>>) {
        let timestamp_prevant_startup = Utc::now();

        tokio::spawn(async move {
            let mut apps_updates = WatchStream::new(apps_updates);
            let mut services = HashMap::with_capacity(0);
            loop {
                // TODO: include shutdown handle which require that the spawn will be called in
                // Rocket's adhoc lift off (see
                // https://api.rocket.rs/v0.5/rocket/struct.Rocket#method.shutdown-1) which
                // requires us to replace evmap (see comment above).
                tokio::select! {
                    Some(new_services) = apps_updates.next() => {
                        services = new_services;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                        if services.is_empty() {
                            continue;
                        }
                    }
                    else => continue,
                };

                if let Some(timestamp) = self
                    .crawl(apps.clone(), &services, timestamp_prevant_startup)
                    .await
                {
                    self.update_watch_tx.send_replace(timestamp);
                }
            }
        });
    }

    async fn crawl(
        &mut self,
        all_apps: Arc<Apps>,
        apps: &HashMap<AppName, Services>,
        since_timestamp: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        self.clear_stale_web_host_meta(apps);

        let services_without_host_meta = apps
            .iter()
            .flat_map(|(app_name, services)| {
                services
                    .iter()
                    // avoid cloning when https://github.com/havarnov/multimap/issues/24 has been implemented
                    .map(move |service| {
                        let key = Key {
                            app_name: app_name.clone(),
                            service_id: service.id().to_string(),
                        };
                        (key, service.clone())
                    })
            })
            .filter(|(key, _service)| !self.writer.contains_key(key))
            .collect::<Vec<(Key, Service)>>();

        if services_without_host_meta.is_empty() {
            return None;
        }

        debug!(
            "Resolving web host meta data for {:?}.",
            services_without_host_meta
                .iter()
                .map(|(k, service)| format!("({}, {})", k.app_name, service.service_name()))
                .fold(String::new(), |a, b| a + &b + ", ")
        );
        let now = Utc::now();
        let duration_prevant_startup = Utc::now().signed_duration_since(since_timestamp);
        let resolved_host_meta_infos = Self::resolve_host_meta(
            all_apps,
            services_without_host_meta,
            duration_prevant_startup,
        )
        .await;
        let mut updated_host_meta_info_entries = 0;
        for (key, _service, web_host_meta) in resolved_host_meta_infos {
            if !web_host_meta.is_valid() {
                continue;
            }

            updated_host_meta_info_entries += 1;

            self.writer.insert(
                key,
                Arc::new(Value {
                    timestamp: now,
                    web_host_meta,
                }),
            );
        }

        self.writer.refresh();

        if updated_host_meta_info_entries > 0 {
            Some(now)
        } else {
            None
        }
    }

    fn clear_stale_web_host_meta(&mut self, apps: &HashMap<AppName, Services>) {
        let copy: HashMap<Key, Vec<_>> = self
            .writer
            .map_into(|k, vs| (k.clone(), vs.iter().cloned().collect()));

        let keys_to_clear = copy
            .into_iter()
            .flat_map(|(key, values)| values.into_iter().map(move |v| (key.clone(), v)))
            .filter(|(key, value)| {
                let service = match apps.get(&key.app_name) {
                    Some(services) => services.iter().find(|s| s.id() == &key.service_id),
                    None => {
                        return true;
                    }
                };

                match service {
                    Some(service) => {
                        *service.status() == ServiceStatus::Paused
                            || *service.started_at() > value.timestamp
                    }
                    None => true,
                }
            })
            .map(|(key, _)| key)
            .collect::<HashSet<Key>>();

        if keys_to_clear.is_empty() {
            return;
        }

        debug!("Clearing stale apps: {:?}", keys_to_clear);

        for key in keys_to_clear {
            self.writer.empty(key);
        }
        self.writer.refresh();
    }

    async fn resolve_host_meta(
        apps: Arc<Apps>,
        services_without_host_meta: Vec<(Key, Service)>,
        duration_prevant_startup: chrono::Duration,
    ) -> Vec<(Key, Service, WebHostMeta)> {
        let number_of_services = services_without_host_meta.len();
        if number_of_services == 0 {
            return Vec::with_capacity(0);
        }

        let mut futures = services_without_host_meta
            .into_iter()
            .map(|(key, service)| async {
                let http_forwarder = match apps.http_forwarder().await {
                    Ok(portforwarder) => portforwarder,
                    Err(err) => {
                        error!(
                            "Cannot forward TCP connection for {}, {}: {err}",
                            key.app_name,
                            service.service_name()
                        );
                        return (key, service, WebHostMeta::empty());
                    }
                };
                Self::resolve_web_host_meta(http_forwarder, key, service, duration_prevant_startup)
                    .await
            })
            .collect::<FuturesUnordered<_>>();

        let mut resolved_host_meta_infos = Vec::with_capacity(number_of_services);
        while let Some(resolved_host_meta) = futures.next().await {
            resolved_host_meta_infos.push(resolved_host_meta);
        }

        resolved_host_meta_infos
    }

    async fn resolve_web_host_meta(
        http_forwarder: Box<dyn HttpForwarder + Send>,
        key: Key,
        service: Service,
        duration_prevant_startup: chrono::Duration,
    ) -> (Key, Service, WebHostMeta) {
        let app_name = &key.app_name;
        let response = http_forwarder
            .request_web_host_meta(
                app_name,
                service.service_name(),
                http::Request::builder()
                    // TODO: include real service traefic route, see #169
                    .header(
                        USER_AGENT.as_str(),
                        format!("PREvant/{}", clap::crate_version!()),
                    )
                    .method("GET")
                    .uri("/.well-known/host-meta.json")
                    .header(HOST, "127.0.0.1")
                    .header("Connection", "Close")
                    .header("Forwarded", "host=www.prevant.example.com;proto=http")
                    .header(
                        "X-Forwarded-Prefix",
                        format!("/{app_name}/{}", service.service_name()),
                    )
                    .header("Accept", "application/json")
                    .body(http_body_util::Empty::<bytes::Bytes>::new())
                    .unwrap(),
            )
            .await;

        let meta = match response {
            Ok(Some(meta)) => {
                debug!(
                    "Got host meta for service {} of {}",
                    Paint::magenta(service.service_name()),
                    Paint::magenta(app_name),
                );
                meta
            }
            Ok(None) => {
                debug!(
                    "Cannot parse host meta for service {} of {}",
                    Paint::magenta(service.service_name()),
                    Paint::magenta(app_name),
                );
                WebHostMeta::empty()
            }
            Err(err) => {
                debug!(
                    "Cannot acquire host meta for service {} of {}: {}",
                    Paint::magenta(service.service_name()),
                    Paint::magenta(app_name),
                    err
                );

                let duration = Utc::now().signed_duration_since(*service.started_at());
                if duration >= chrono::Duration::minutes(5)
                    && duration_prevant_startup >= chrono::Duration::minutes(1)
                {
                    info!(
                        "Service {} is running for {}, therefore, it will be assumed that host-meta.json is not available.",
                        Paint::magenta(service.service_name()), duration
                    );
                    WebHostMeta::empty()
                } else {
                    WebHostMeta::invalid()
                }
            }
        };
        (key, service, meta)
    }
    #[cfg(test)]
    pub fn fake_empty_host_meta_info(&mut self, app_name: AppName, service_id: String) {
        let web_host_meta = WebHostMeta::empty();
        let value = Arc::new(Value {
            timestamp: chrono::Utc::now(),
            web_host_meta,
        });

        self.writer.insert(
            Key {
                app_name,
                service_id,
            },
            value,
        );

        self.writer.refresh();
        self.writer.flush();
    }
}
