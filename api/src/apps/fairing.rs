use crate::{
    apps::Apps,
    config::Config,
    infrastructure::{Infrastructure, TraefikIngressRoute},
    models::{App, AppName},
};
use rocket::{
    fairing::{Fairing, Info, Kind},
    Build, Orbit, Rocket,
};
use std::{collections::HashMap, sync::Mutex};
use tokio::sync::watch::Receiver;

pub struct AppsFairing {
    infrastructure: Mutex<Option<Box<dyn Infrastructure + Send>>>,
    config: Config,
    host_meta_cache: Mutex<Option<super::host_meta_cache::HostMetaCache>>,
    host_meta_crawler: Mutex<Option<super::host_meta_cache::HostMetaCrawler>>,
}

impl Apps {
    pub fn fairing(config: Config, infrastructure: Box<dyn Infrastructure>) -> AppsFairing {
        let (host_meta_cache, host_meta_crawler) = super::host_meta_crawling(config.clone());
        AppsFairing {
            infrastructure: Mutex::new(Some(infrastructure)),
            config,
            host_meta_cache: Mutex::new(Some(host_meta_cache)),
            host_meta_crawler: Mutex::new(Some(host_meta_crawler)),
        }
    }
}

#[rocket::async_trait]
impl Fairing for AppsFairing {
    fn info(&self) -> Info {
        Info {
            name: "apps",
            kind: Kind::Ignite | Kind::Liftoff,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<Build>) -> rocket::fairing::Result {
        let prevant_base_route = rocket.state::<Option<TraefikIngressRoute>>();

        let infrastructure = {
            let mut infrastructure = self.infrastructure.lock().unwrap();
            infrastructure.take().expect("Should be available")
        };

        let apps = match Apps::new(self.config.clone(), infrastructure) {
            Ok(apps) => apps.with_base_route(prevant_base_route.cloned().flatten()),
            Err(_err) => {
                // TODO: message
                return Err(rocket);
            }
        };

        let host_meta_cache = {
            let mut host_meta_cache = self.host_meta_cache.lock().unwrap();
            host_meta_cache.take().expect("TODO")
        };

        let app_updates = apps.app_updates().await;

        Ok(rocket
            .manage(apps)
            .manage(host_meta_cache)
            .manage(app_updates))
    }

    async fn on_liftoff(&self, rocket: &Rocket<Orbit>) {
        let host_meta_crawler = {
            let mut host_meta_cache = self.host_meta_crawler.lock().unwrap();
            host_meta_cache.take().unwrap()
        };

        let apps = rocket.state::<Apps>().unwrap();
        let app_updates = rocket.state::<Receiver<HashMap<AppName, App>>>().unwrap();

        let http_forwarder = match apps.http_forwarder().await {
            Ok(http_forwarder) => http_forwarder,
            Err(err) => {
                log::error!("Cannot acquire http forwarder for crawling web host meta: {err}");
                rocket.shutdown().notify();
                return;
            }
        };

        host_meta_crawler.spawn(http_forwarder, app_updates.clone());
    }
}
