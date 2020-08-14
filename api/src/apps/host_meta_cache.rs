use crate::apps::Apps;
use crate::models::service::{Service, ServiceBuilder};
use crate::models::RequestInfo;
use crate::models::WebHostMeta;

use chrono::Utc;
use evmap::ReadHandle;
use evmap::WriteHandle;
use multimap::MultiMap;
use std::convert::From;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use url::Url;
use yansi::Paint;

pub struct HostMetaCache {
    reader: ReadHandle<(String, String), Arc<WebHostMeta>>,
}
pub struct HostMetaCrawler {
    writer: WriteHandle<(String, String), Arc<WebHostMeta>>,
}

pub fn new() -> (HostMetaCache, HostMetaCrawler) {
    let (reader, writer) = evmap::new();

    (HostMetaCache { reader }, HostMetaCrawler { writer })
}

impl HostMetaCache {
    pub fn update_meta_data(
        &self,
        services: MultiMap<String, Service>,
        request_info: &RequestInfo,
    ) -> MultiMap<String, Service> {
        let mut assigned_apps = MultiMap::new();

        for (app_name, service) in services.iter_all() {
            for service in service.iter().cloned() {
                let tuple = (app_name.clone(), service.id().clone());
                let mut b = ServiceBuilder::from(service);
                if let Some(web_host_meta) = self.reader.get_one(&tuple) {
                    b = b.web_host_meta(web_host_meta.with_base_url(request_info.get_base_url()));
                }
                assigned_apps.insert(app_name.clone(), b.build().unwrap());
            }
        }
        assigned_apps
    }
}

impl HostMetaCrawler {
    pub fn spawn(mut self, apps: Arc<Apps>) {
        let mut runtime = Runtime::new().expect("Should create runtime");

        std::thread::spawn(move || loop {
            let services = match apps.get_apps() {
                Ok(apps) => {
                    info!("apps: {:?}", apps);
                    apps.iter_all()
                        .flat_map(|(app_name, services)| {
                            services
                                .iter()
                                // avoid cloning when https://github.com/havarnov/multimap/issues/24 has been implemented
                                .map(move |service| (app_name.clone(), service.clone()))
                        })
                        .collect::<Vec<_>>()
                }
                Err(error) => {
                    error!("error: {}", error);
                    continue;
                }
            };

            let resolved_host_meta_infos = runtime.block_on(Self::resolve_host_meta(services));

            info!("resolved infos: {:?}", resolved_host_meta_infos);
            self.writer.purge();
            for (app_name, service, web_host_meta) in resolved_host_meta_infos {
                self.writer
                    .insert((app_name, service.id().clone()), Arc::new(web_host_meta));
            }
            self.writer.refresh();

            std::thread::sleep(std::time::Duration::from_secs(5));
        });
    }

    async fn resolve_host_meta(
        services_without_host_meta: Vec<(String, Service)>,
    ) -> Vec<(String, Service, WebHostMeta)> {
        let number_of_services = services_without_host_meta.len();
        if number_of_services == 0 {
            return Vec::with_capacity(0);
        }

        trace!("Resolve web host meta for {} services.", number_of_services);

        let (tx, mut rx) = mpsc::channel(number_of_services);

        for (app_name, service) in services_without_host_meta {
            let mut tx = tx.clone();
            tokio::spawn(async move {
                let r = Self::resolve_web_host_meta(app_name, service).await;
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
        app_name: String,
        service: Service,
    ) -> (String, Service, WebHostMeta) {
        let url = match service.endpoint_url() {
            None => return (app_name, service, WebHostMeta::empty()),
            Some(endpoint_url) => endpoint_url.join(".well-known/host-meta.json").unwrap(),
        };

        let request_info = RequestInfo::new(Url::parse("http://www.prevant-example.de").unwrap());
        let get_request = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_millis(750))
            .user_agent(format!("PREvant/{}", crate_version!()))
            .build()
            .unwrap()
            .get(&url.to_string())
            .header(
                "Forwarded",
                format!(
                    "host={};proto={}",
                    request_info.host(),
                    request_info.scheme()
                ),
            )
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

                let duration = Utc::now().signed_duration_since(service.started_at().clone());
                if duration >= chrono::Duration::minutes(5) {
                    trace!(
                        "Service {} is running for {}, therefore, it will be assumed that host-meta.json is not available.",
                        Paint::magenta(service.service_name()), duration
                    );
                    WebHostMeta::empty()
                } else {
                    WebHostMeta::invalid()
                }
            }
        };
        (app_name, service, meta)
    }
}
