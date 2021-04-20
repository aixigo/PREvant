use crate::apps::apps::AppsServiceError;
use crate::apps::Apps;
use crate::apps::RUNTIME as runtime;
use crate::models::service::{Service, ServiceBuilder, ServiceStatus};
use crate::models::RequestInfo;
use crate::models::WebHostMeta;
use chrono::{DateTime, Utc};
use evmap::{ReadHandleFactory, WriteHandle};
use multimap::MultiMap;
use std::collections::{HashMap, HashSet};
use std::convert::From;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use yansi::Paint;

pub struct HostMetaCache {
    reader_factory: ReadHandleFactory<Key, Arc<Value>>,
}
pub struct HostMetaCrawler {
    writer: WriteHandle<Key, Arc<Value>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    app_name: String,
    service_id: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Value {
    timestamp: DateTime<Utc>,
    web_host_meta: WebHostMeta,
}

pub fn new() -> (HostMetaCache, HostMetaCrawler) {
    let (reader, writer) = evmap::new();

    (
        HostMetaCache {
            reader_factory: reader.factory(),
        },
        HostMetaCrawler { writer },
    )
}

impl HostMetaCache {
    pub fn update_meta_data(
        &self,
        services: MultiMap<String, Service>,
        request_info: &RequestInfo,
    ) -> MultiMap<String, Service> {
        let mut assigned_apps = MultiMap::new();

        let reader = self.reader_factory.handle();

        for (app_name, service) in services.iter_all() {
            for service in service.iter().cloned() {
                let key = Key {
                    app_name: app_name.to_string(),
                    service_id: service.id().to_string(),
                };

                let mut b =
                    ServiceBuilder::from(service).base_url(request_info.get_base_url().clone());
                if let Some(value) = reader.get_one(&key) {
                    b = b.web_host_meta(
                        value
                            .web_host_meta
                            .with_base_url(request_info.get_base_url()),
                    );
                }

                assigned_apps.insert(key.app_name, b.build().unwrap());
            }
        }

        assigned_apps
    }
}

impl HostMetaCrawler {
    pub fn spawn(mut self, apps: Arc<Apps>) {
        let timestamp_prevant_startup = Utc::now();

        runtime.handle().spawn(async move {
            loop {
                sleep(Duration::from_secs(5)).await;
                if let Err(err) = self.crawl(&apps, timestamp_prevant_startup).await {
                    error!("Cannot load apps: {}", err);
                }
            }
        });
    }

    async fn crawl(
        &mut self,
        apps: &Arc<Apps>,
        since_timestamp: DateTime<Utc>,
    ) -> Result<(), AppsServiceError> {
        debug!("Resolving list of apps for web host meta cache.");
        let apps = apps.get_apps().await?;

        self.clear_stale_web_host_meta(&apps);

        let services_without_host_meta = apps
            .iter_all()
            .flat_map(|(app_name, services)| {
                services
                    .iter()
                    // avoid cloning when https://github.com/havarnov/multimap/issues/24 has been implemented
                    .map(move |service| {
                        let key = Key {
                            app_name: app_name.to_string(),
                            service_id: service.id().to_string(),
                        };
                        (key, service.clone())
                    })
            })
            .filter(|(key, _service)| !self.writer.contains_key(key))
            .collect::<Vec<(Key, Service)>>();

        if services_without_host_meta.is_empty() {
            return Ok(());
        }

        debug!(
            "Resolving web host meta data for {:?}.",
            services_without_host_meta
                .iter()
                .map(|(k, _)| k)
                .collect::<Vec<_>>()
        );
        let now = Utc::now();
        let duration_prevant_startup = Utc::now().signed_duration_since(since_timestamp);
        let resolved_host_meta_infos =
            Self::resolve_host_meta(services_without_host_meta, duration_prevant_startup).await;
        for (key, _service, web_host_meta) in resolved_host_meta_infos {
            if !web_host_meta.is_valid() {
                continue;
            }

            self.writer.insert(
                key,
                Arc::new(Value {
                    timestamp: now,
                    web_host_meta,
                }),
            );
        }

        self.writer.refresh();
        Ok(())
    }

    fn clear_stale_web_host_meta(&mut self, apps: &MultiMap<String, Service>) {
        let copy: HashMap<Key, Vec<_>> = self
            .writer
            .map_into(|k, vs| (k.clone(), vs.iter().cloned().collect()));

        let keys_to_clear = copy
            .into_iter()
            .flat_map(|(key, values)| values.into_iter().map(move |v| (key.clone(), v)))
            .filter(|(key, value)| {
                let service = match apps.get_vec(&key.app_name) {
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
        services_without_host_meta: Vec<(Key, Service)>,
        duration_prevant_startup: chrono::Duration,
    ) -> Vec<(Key, Service, WebHostMeta)> {
        let number_of_services = services_without_host_meta.len();
        if number_of_services == 0 {
            return Vec::with_capacity(0);
        }

        let (tx, mut rx) = mpsc::channel(number_of_services);

        for (key, service) in services_without_host_meta {
            let tx = tx.clone();
            tokio::spawn(async move {
                let r = Self::resolve_web_host_meta(key, service, duration_prevant_startup).await;
                if let Err(err) = tx.send(r).await {
                    error!("Cannot send host meta result: {}", err);
                }
            });
        }

        let mut resolved_host_meta_infos = Vec::with_capacity(number_of_services);
        for _c in 0..number_of_services {
            let resolved_host_meta = rx.recv().await.unwrap();
            resolved_host_meta_infos.push(resolved_host_meta);
        }

        resolved_host_meta_infos
    }

    async fn resolve_web_host_meta(
        key: Key,
        service: Service,
        duration_prevant_startup: chrono::Duration,
    ) -> (Key, Service, WebHostMeta) {
        let url = match service.endpoint_url() {
            None => return (key, service, WebHostMeta::invalid()),
            Some(endpoint_url) => endpoint_url.join(".well-known/host-meta.json").unwrap(),
        };

        let get_request = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_millis(750))
            .user_agent(format!("PREvant/{}", crate_version!()))
            .build()
            .unwrap()
            .get(&url.to_string())
            .header("Forwarded", "host=www.prevant.example.com;proto=http")
            .header(
                "X-Forwarded-Prefix",
                format!("/{}/{}", service.app_name(), service.service_name()),
            )
            .header("Accept", "application/json")
            .send()
            .await;

        let meta = match get_request {
            Ok(response) => match response.json::<WebHostMeta>().await {
                Ok(meta) => meta,
                Err(err) => {
                    error!(
                        "Cannot parse host meta for service {} of {}: {}",
                        Paint::magenta(service.service_name()),
                        Paint::magenta(service.app_name()),
                        err
                    );
                    WebHostMeta::empty()
                }
            },
            Err(err) => {
                debug!(
                    "Cannot acquire host meta for service {} of {}: {}",
                    Paint::magenta(service.service_name()),
                    Paint::magenta(service.app_name()),
                    err
                );

                let duration = Utc::now().signed_duration_since(*service.started_at());
                if duration >= chrono::Duration::minutes(5)
                    && duration_prevant_startup >= chrono::Duration::minutes(1)
                {
                    error!(
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
}
