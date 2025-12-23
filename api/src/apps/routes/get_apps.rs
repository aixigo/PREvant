use crate::{
    apps::{
        repository::{AppPostgresRepository, BackupUpdateReceiver},
        routes::AppV2,
        Apps, HostMetaCache,
    },
    http_result::{HttpApiError, HttpResult},
    models::{App, AppName, AppWithHostMeta, RequestInfo},
};
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use rocket::{
    response::stream::{Event, EventStream},
    serde::json::Json,
    Shutdown, State,
};
use serde::{ser::SerializeMap as _, Serialize};
use std::collections::HashMap;
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

pub(super) struct AppsV2(HashMap<AppName, AppV2>);

impl Serialize for AppsV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;

        for (app_name, app) in self.0.iter() {
            map.serialize_entry(app_name, app)?;
        }

        map.end()
    }
}

#[rocket::get("/", format = "application/json", rank = 1)]
pub(super) async fn apps_v1(
    apps: &State<Apps>,
    request_info: RequestInfo,
    host_meta_cache: &State<HostMetaCache>,
) -> HttpResult<Json<AppsV1>> {
    // We don't fetch app backups here because the deprecated API wouldn't have an option to
    // show the outside what kind of application the consumer received. Seeing the backed up
    // applications on the receivers' ends would be a semantic breaking change.
    let apps = apps.fetch_apps().await?;
    Ok(Json(AppsV1(
        host_meta_cache.assign_host_meta_data(apps, &request_info),
    )))
}

fn merge(
    deployed_apps: HashMap<AppName, AppWithHostMeta>,
    backed_up_apps: HashMap<AppName, App>,
) -> HashMap<AppName, AppV2> {
    let mut deployed_apps = deployed_apps
        .into_iter()
        .map(|(app_name, app)| (app_name, AppV2::Deployed(app)))
        .collect::<HashMap<_, _>>();

    let backed_up_apps = backed_up_apps
        .into_iter()
        .map(|(app_name, app)| (app_name, AppV2::BackedUp(app)))
        .collect::<HashMap<_, _>>();

    deployed_apps.extend(backed_up_apps);

    deployed_apps
}

#[rocket::get("/", format = "application/vnd.prevant.v2+json", rank = 2)]
pub(super) async fn apps_v2(
    apps: &State<Apps>,
    app_repository: &State<Option<AppPostgresRepository>>,
    request_info: RequestInfo,
    host_meta_cache: &State<HostMetaCache>,
) -> HttpResult<Json<AppsV2>> {
    let (apps, app_backups) = futures::try_join!(
        async {
            apps.fetch_apps()
                .await
                .map_err(HttpApiError::from)
                .map(|apps| host_meta_cache.assign_host_meta_data(apps, &request_info))
        },
        async {
            match &**app_repository {
                Some(app_repository) => app_repository.fetch_backed_up_apps().await.map_err(|e| {
                    HttpApiError::from(
                        HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                            .detail(e.to_string()),
                    )
                }),
                None => Ok(HashMap::new()),
            }
        }
    )?;

    Ok(Json(AppsV2(merge(apps, app_backups))))
}

#[rocket::get("/", format = "text/event-stream", rank = 3)]
pub(super) async fn stream_apps_v1(
    apps_updates: &State<Receiver<HashMap<AppName, App>>>,
    mut end: Shutdown,
    request_info: RequestInfo,
    host_meta_cache: HostMetaCache,
) -> EventStream![] {
    // We don't fetch app backups here because the deprecated API wouldn't have an option to
    // show the outside what kind of application the consumer received. Seeing the backed up
    // applications on the receivers' ends would be a semantic breaking change.
    let mut deployed_apps = apps_updates.inner().borrow().clone();

    let mut app_changes =
        tokio_stream::wrappers::WatchStream::from_changes(apps_updates.inner().clone());
    let mut host_meta_cache_updates = host_meta_cache.cache_updates();

    EventStream! {
        yield Event::json(&AppsV1(host_meta_cache.assign_host_meta_data(deployed_apps.clone(), &request_info)));

        loop {
            select! {
                Some(new_apps) = app_changes.next() => {
                    log::debug!("New app list update: sending app service update");
                    deployed_apps = new_apps;
                }
                Some(_t) = host_meta_cache_updates.next() => {
                    log::debug!("New host meta cache update: sending app service update");
                }
                _ = &mut end => break,
            };

            yield Event::json(&AppsV1(host_meta_cache.assign_host_meta_data(deployed_apps.clone(), &request_info)));
        }
    }
}

#[rocket::get("/", format = "text/vnd.prevant.v2+event-stream", rank = 4)]
pub(super) async fn stream_apps_v2(
    apps_updates: &State<Receiver<HashMap<AppName, App>>>,
    backup_updates: &State<Option<BackupUpdateReceiver>>,
    mut end: Shutdown,
    request_info: RequestInfo,
    host_meta_cache: HostMetaCache,
) -> EventStream![] {
    let mut deployed_apps = apps_updates.inner().borrow().clone();
    let mut backed_up_apps = match &**backup_updates {
        Some(backup_updates) => backup_updates.0.borrow().clone(),
        None => HashMap::new(),
    };

    let mut app_changes =
        tokio_stream::wrappers::WatchStream::from_changes(apps_updates.inner().clone());
    let mut backup_changes = match &**backup_updates {
        Some(backup_updates) => {
            tokio_stream::wrappers::WatchStream::from_changes(backup_updates.0.clone())
        }
        None => {
            let (_tx, rx) = tokio::sync::watch::channel(HashMap::new());
            tokio_stream::wrappers::WatchStream::from_changes(rx)
        }
    };
    let mut host_meta_cache_updates = host_meta_cache.cache_updates();
    EventStream! {
        yield Event::json(&AppsV2(merge(
            host_meta_cache.assign_host_meta_data(deployed_apps.clone(), &request_info),
            backed_up_apps.clone(),
        )));

        loop {
            select! {
                Some(new_apps) = app_changes.next() => {
                    log::debug!("New app list update: sending app service update");
                    deployed_apps = new_apps;
                }
                Some(new_backups) = backup_changes.next() => {
                    log::debug!("New backup list update: sending app service update");
                    backed_up_apps = new_backups;
                }
                Some(_t) = host_meta_cache_updates.next() => {
                    log::debug!("New host meta cache update: sending app service update");
                }
                _ = &mut end => break,
            };

            yield Event::json(&AppsV2(merge(
                host_meta_cache.assign_host_meta_data(deployed_apps.clone(), &request_info),
                backed_up_apps.clone(),
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Owner, Service, ServiceStatus, ServiceWithHostMeta, WebHostMeta};
    use assert_json_diff::assert_json_eq;
    use chrono::Utc;
    use std::{collections::HashSet, str::FromStr};
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
                    "status": "deployed",
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
                AppV2::Deployed(AppWithHostMeta::new(
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
                ))
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
                    "status": "deployed",
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
                AppV2::Deployed(AppWithHostMeta::new(
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
                ))
            )])))
            .unwrap()
        );
    }

    mod url_rendering {
        use super::apps_v1;
        use crate::apps::repository::{AppPostgresRepository, BackupUpdateReceiver};
        use crate::apps::{AppProcessingQueue, Apps, HostMetaCache};
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

        async fn set_up_rocket_with_dummy_infrastructure_and_a_running_app(
            host_meta_cache: HostMetaCache,
        ) -> Result<Client, crate::apps::AppsError> {
            let infrastructure = Box::new(Dummy::new());
            let apps = Apps::new(Default::default(), infrastructure).unwrap();
            let _result = apps
                .create_or_update(&AppName::master(), None, &[sc!("service-a")], vec![], None)
                .await?;

            let rocket = rocket::build()
                .manage(host_meta_cache)
                .manage(apps)
                .manage(Config::default())
                .manage(None::<BackupUpdateReceiver>)
                .manage(None::<AppPostgresRepository>)
                .manage(tokio::sync::watch::channel::<HashMap<AppName, App>>(HashMap::new()).1)
                .mount("/", rocket::routes![apps_v1])
                .mount("/api/apps", crate::apps::apps_routes())
                .attach(AppProcessingQueue::fairing());
            Ok(Client::tracked(rocket).await.expect("valid rocket"))
        }

        #[tokio::test]
        async fn host_header_response_with_xforwardedhost_and_port_xforwardedproto_and_xforwardedport(
        ) -> Result<(), crate::apps::AppsError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::apps::host_meta_crawling(Config::default());
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
        ) -> Result<(), crate::apps::AppsError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::apps::host_meta_crawling(Config::default());
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
        ) -> Result<(), crate::apps::AppsError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::apps::host_meta_crawling(Config::default());
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
        ) -> Result<(), crate::apps::AppsError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::apps::host_meta_crawling(Config::default());
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
        ) -> Result<(), crate::apps::AppsError> {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::apps::host_meta_crawling(Config::default());
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
        async fn host_header_response_with_all_default_values() -> Result<(), crate::apps::AppsError>
        {
            let (host_meta_cache, mut host_meta_crawler) =
                crate::apps::host_meta_crawling(Config::default());
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
