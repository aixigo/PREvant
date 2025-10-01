use crate::{
    apps::{Apps, HostMetaCache},
    http_result::HttpResult,
    models::{App, AppName, AppWithHostMeta, Owner, RequestInfo, ServiceWithHostMeta},
};
use rocket::{
    response::stream::{Event, EventStream},
    serde::json::Json,
    Shutdown, State,
};
use serde::{ser::SerializeMap as _, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::{select, sync::watch::Receiver};
use tokio_stream::StreamExt;

pub(super) struct AppsV1(HashMap<AppName, AppWithHostMeta>);

impl Serialize for AppsV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;

        for (app_name, services_with_host_meta) in self.0.iter() {
            map.serialize_entry(app_name, services_with_host_meta.services())?;
        }

        map.end()
    }
}

pub(super) struct AppsV2(HashMap<AppName, AppWithHostMeta>);

impl Serialize for AppsV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;

        #[derive(Serialize)]
        struct App<'a> {
            services: &'a [ServiceWithHostMeta],
            #[serde(skip_serializing_if = "HashSet::is_empty")]
            owners: &'a HashSet<Owner>,
        }

        for (app_name, services_with_host_meta) in self.0.iter() {
            map.serialize_entry(
                app_name,
                &App {
                    services: services_with_host_meta.services(),
                    owners: services_with_host_meta.owners(),
                },
            )?;
        }

        map.end()
    }
}

#[rocket::get("/", format = "application/json", rank = 1)]
pub(super) async fn apps_v1(
    apps: &State<Arc<Apps>>,
    request_info: RequestInfo,
    host_meta_cache: &State<HostMetaCache>,
) -> HttpResult<Json<AppsV1>> {
    let apps = apps.fetch_apps().await?;
    Ok(Json(AppsV1(
        host_meta_cache.assign_host_meta_data(apps, &request_info),
    )))
}

#[rocket::get("/", format = "application/vnd.prevant.v2+json", rank = 2)]
pub(super) async fn apps_v2(
    apps: &State<Arc<Apps>>,
    request_info: RequestInfo,
    host_meta_cache: &State<HostMetaCache>,
) -> HttpResult<Json<AppsV2>> {
    let apps = apps.fetch_apps().await?;
    Ok(Json(AppsV2(
        host_meta_cache.assign_host_meta_data(apps, &request_info),
    )))
}

macro_rules! stream_apps {
    ($apps_updates:ident, $host_meta_cache:ident, $request_info:ident, $end:ident, $app_version_type:ty) => {{
        let mut services = $apps_updates.inner().borrow().clone();

        let mut app_changes =
            tokio_stream::wrappers::WatchStream::from_changes($apps_updates.inner().clone());
        let mut host_meta_cache_updates = $host_meta_cache.cache_updates();

        EventStream! {
            yield Event::json(&$app_version_type($host_meta_cache.assign_host_meta_data(services.clone(), &$request_info)));

            loop {
                select! {
                    Some(new_services) = app_changes.next() => {
                        log::debug!("New app list update: sending app service update");
                        services = new_services;
                    }
                    Some(_t) = host_meta_cache_updates.next() => {
                        log::debug!("New host meta cache update: sending app service update");
                    }
                    _ = &mut $end => break,
                };

                yield Event::json(&$app_version_type($host_meta_cache.assign_host_meta_data(services.clone(), &$request_info)));
            }
        }
    }};
}

#[rocket::get("/", format = "text/event-stream", rank = 3)]
pub(super) async fn stream_apps_v1(
    apps_updates: &State<Receiver<HashMap<AppName, App>>>,
    mut end: Shutdown,
    request_info: RequestInfo,
    host_meta_cache: HostMetaCache,
) -> EventStream![] {
    stream_apps!(apps_updates, host_meta_cache, request_info, end, AppsV1)
}

#[rocket::get("/", format = "text/vnd.prevant.v2+event-stream", rank = 4)]
pub(super) async fn stream_apps_v2(
    apps_updates: &State<Receiver<HashMap<AppName, App>>>,
    mut end: Shutdown,
    request_info: RequestInfo,
    host_meta_cache: HostMetaCache,
) -> EventStream![] {
    stream_apps!(apps_updates, host_meta_cache, request_info, end, AppsV2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Service, ServiceStatus, ServiceWithHostMeta, WebHostMeta};
    use assert_json_diff::assert_json_eq;
    use chrono::Utc;
    use std::str::FromStr;
    use url::Url;

    #[test]
    fn serialize_v1_apps_with_web_host_meta() {
        let base_url = Url::from_str("http://prevant.example.com").unwrap();
        let app_name = AppName::master();

        assert_json_eq!(
            serde_json::json!({
                "master": [{
                    "name": "mariadb",
                    "type": "instance",
                    "state": {
                        "status": "running"
                    },
                    "url": "http://prevant.example.com/master/mariadb/",
                    "version": {
                        "softwareVersion": "1.2.3"
                    }
                }, {
                    "name": "postgres",
                    "type": "instance",
                    "state": {
                        "status": "running"
                    }
                }]
            }),
            serde_json::to_value(AppsV1(HashMap::from([(
                app_name.clone(),
                AppWithHostMeta::new(
                    vec![
                        ServiceWithHostMeta::from_service_and_web_host_meta(
                            Service {
                                id: String::from("some id"),
                                state: crate::models::State {
                                    status: ServiceStatus::Running,
                                    started_at: Some(Utc::now()),
                                },
                                config: crate::sc!("postgres", "postgres:latest")
                            },
                            WebHostMeta::invalid(),
                            base_url.clone(),
                            &app_name
                        ),
                        ServiceWithHostMeta::from_service_and_web_host_meta(
                            Service {
                                id: String::from("some id"),
                                state: crate::models::State {
                                    status: ServiceStatus::Running,
                                    started_at: Some(Utc::now()),
                                },
                                config: crate::sc!("mariadb", "mariadb:latest")
                            },
                            WebHostMeta::with_version(String::from("1.2.3")),
                            base_url,
                            &app_name
                        )
                    ],
                    HashSet::new()
                )
            )])))
            .unwrap()
        );
    }

    #[test]
    fn serialize_v2_apps_with_web_host_meta() {
        let base_url = Url::from_str("http://prevant.example.com").unwrap();
        let app_name = AppName::master();

        assert_json_eq!(
            serde_json::json!({
                "master": {
                    "services": [{
                        "name": "mariadb",
                        "type": "instance",
                        "state": {
                            "status": "running"
                        },
                        "url": "http://prevant.example.com/master/mariadb/",
                        "version": {
                            "softwareVersion": "1.2.3"
                        }
                    }, {
                        "name": "postgres",
                        "type": "instance",
                        "state": {
                            "status": "running"
                        }
                    }]
                }
            }),
            serde_json::to_value(AppsV2(HashMap::from([(
                app_name.clone(),
                AppWithHostMeta::new(
                    vec![
                        ServiceWithHostMeta::from_service_and_web_host_meta(
                            Service {
                                id: String::from("some id"),
                                state: crate::models::State {
                                    status: ServiceStatus::Running,
                                    started_at: Some(Utc::now()),
                                },
                                config: crate::sc!("postgres", "postgres:latest")
                            },
                            WebHostMeta::invalid(),
                            base_url.clone(),
                            &app_name
                        ),
                        ServiceWithHostMeta::from_service_and_web_host_meta(
                            Service {
                                id: String::from("some id"),
                                state: crate::models::State {
                                    status: ServiceStatus::Running,
                                    started_at: Some(Utc::now()),
                                },
                                config: crate::sc!("mariadb", "mariadb:latest")
                            },
                            WebHostMeta::with_version(String::from("1.2.3")),
                            base_url,
                            &app_name
                        )
                    ],
                    HashSet::new()
                )
            )])))
            .unwrap()
        );
    }

    #[test]
    fn serialize_v2_apps_with_web_host_meta_and_owners() {
        let base_url = Url::from_str("http://prevant.example.com").unwrap();
        let app_name = AppName::master();

        assert_json_eq!(
            serde_json::json!({
                "master": {
                    "owners": [{
                        "sub": "some-sub",
                        "iss": "https://openid.example.com"
                    }],
                    "services": [{
                        "name": "mariadb",
                        "type": "instance",
                        "state": {
                            "status": "running"
                        },
                        "url": "http://prevant.example.com/master/mariadb/",
                        "version": {
                            "softwareVersion": "1.2.3"
                        }
                    }, {
                        "name": "postgres",
                        "type": "instance",
                        "state": {
                            "status": "running"
                        }
                    }]
                }
            }),
            serde_json::to_value(AppsV2(HashMap::from([(
                app_name.clone(),
                AppWithHostMeta::new(
                    vec![
                        ServiceWithHostMeta::from_service_and_web_host_meta(
                            Service {
                                id: String::from("some id"),
                                state: crate::models::State {
                                    status: ServiceStatus::Running,
                                    started_at: Some(Utc::now()),
                                },
                                config: crate::sc!("postgres", "postgres:latest")
                            },
                            WebHostMeta::invalid(),
                            base_url.clone(),
                            &app_name
                        ),
                        ServiceWithHostMeta::from_service_and_web_host_meta(
                            Service {
                                id: String::from("some id"),
                                state: crate::models::State {
                                    status: ServiceStatus::Running,
                                    started_at: Some(Utc::now()),
                                },
                                config: crate::sc!("mariadb", "mariadb:latest")
                            },
                            WebHostMeta::with_version(String::from("1.2.3")),
                            base_url,
                            &app_name
                        )
                    ],
                    HashSet::from([Owner {
                        sub: openidconnect::SubjectIdentifier::new(String::from("some-sub")),
                        iss: openidconnect::IssuerUrl::new(String::from(
                            "https://openid.example.com"
                        ))
                        .unwrap(),
                        name: None,
                    }])
                )
            )])))
            .unwrap()
        );
    }

    mod url_rendering {
        use super::apps_v1;
        use crate::apps::{AppProcessingQueue, AppsService, HostMetaCache};
        use crate::config::Config;
        use crate::infrastructure::Dummy;
        use crate::models::{App, AppName};
        use crate::sc;
        use assert_json_diff::assert_json_include;
        use rocket::http::ContentType;
        use rocket::local::asynchronous::Client;
        use serde_json::json;
        use serde_json::Value;
        use std::collections::HashMap;
        use std::convert::From;
        use std::sync::Arc;

        async fn set_up_rocket_with_dummy_infrastructure_and_a_running_app(
            host_meta_cache: HostMetaCache,
        ) -> Result<Client, crate::apps::AppsServiceError> {
            let infrastructure = Box::new(Dummy::new());
            let apps = Arc::new(AppsService::new(Default::default(), infrastructure).unwrap());
            let _result = apps
                .create_or_update(
                    &AppName::master(),
                    None,
                    &vec![sc!("service-a")],
                    vec![],
                    None,
                )
                .await?;

            let rocket = rocket::build()
                .manage(host_meta_cache)
                .manage(apps)
                .manage(Config::default())
                .manage(tokio::sync::watch::channel::<HashMap<AppName, App>>(HashMap::new()).1)
                .mount("/", rocket::routes![apps_v1])
                .mount("/api/apps", crate::apps::apps_routes())
                .attach(AppProcessingQueue::fairing());
            Ok(Client::tracked(rocket).await.expect("valid rocket"))
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedhost_and_port_xforwardedproto_and_xforwardedport(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::host_meta_crawling(Config::default());
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new(
                    "x-forwarded-host",
                    "prevant.com:8433",
                ))
                .header(rocket::http::Header::new("x-forwarded-proto", "http"))
                .header(rocket::http::Header::new("x-forwarded-port", "8433"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();

            assert_json_include!(
                actual: value_in_json,
                expected: json!({
                    "master": [{
                        "url":"http://prevant.com:8433/master/service-a/"
                    }]
                }
            ));

            Ok(())
        }
        #[tokio::test]
        async fn host_header_response_with_xforwardedhost_xforwardedproto_and_xforwardedport(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::host_meta_crawling(Config::default());
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new("x-forwarded-host", "prevant.com"))
                .header(rocket::http::Header::new("x-forwarded-proto", "http"))
                .header(rocket::http::Header::new("x-forwarded-port", "8433"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();

            assert_json_include!(
                actual: value_in_json,
                expected: json!({
                    "master": [{
                        "url":"http://prevant.com:8433/master/service-a/"
                    }]
                }
            ));

            Ok(())
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedproto_and_other_default_values(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::host_meta_crawling(Config::default());
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new("x-forwarded-proto", "https"))
                .header(rocket::http::Header::new("host", "localhost"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();
            assert_json_include!(
                actual: value_in_json,
                expected: json!({
                    "master": [{
                        "url":"https://localhost/master/service-a/"
                    }]
                }
            ));

            Ok(())
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedhost_and_other_default_values(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::host_meta_crawling(Config::default());
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new("x-forwarded-host", "prevant.com"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();
            assert_json_include!(
                actual: value_in_json,
                expected: json!({
                    "master": [{
                        "url":"http://prevant.com/master/service-a/"
                    }]
                }
            ));

            Ok(())
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedport_and_default_values(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::host_meta_crawling(Config::default());
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());

            let get = client
                .get("/")
                .header(rocket::http::Header::new("host", "localhost"))
                .header(rocket::http::Header::new("x-forwarded-port", "8433"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();
            assert_json_include!(
                actual: value_in_json,
                expected: json!({
                    "master": [{
                        "url":"http://localhost:8433/master/service-a/"
                    }]
                }
            ));

            Ok(())
        }

        #[tokio::test]
        async fn host_header_response_with_all_default_values(
        ) -> Result<(), crate::apps::AppsServiceError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::host_meta_crawling(Config::default());
            let client =
                set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;
            host_meta_crawler.fake_empty_host_meta_info(AppName::master(), "service-a".to_string());
            let get = client
                .get("/")
                .header(rocket::http::Header::new("host", "localhost"))
                .header(ContentType::JSON)
                .dispatch();

            let response = get.await;

            let body_str = response.into_string().await.expect("valid response body");
            let value_in_json: Value = serde_json::from_str(&body_str).unwrap();
            assert_json_include!(
                actual: value_in_json,
                expected: json!({
                    "master": [{
                        "url":"http://localhost/master/service-a/"
                    }]
                }
            ));

            Ok(())
        }
    }
}
