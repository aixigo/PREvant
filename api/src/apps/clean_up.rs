use crate::{
    apps::{repository::AppPostgresRepository, AppTaskQueueProducer, Apps},
    config::{ApplicationCleanUpPolicy, Config},
    models::AppName,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::try_join;
use regex::Regex;
use rocket::{
    fairing::{Fairing, Info, Kind},
    Build, Orbit, Rocket,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::AtomicBool,
    time::Duration,
};

pub struct AppCleanUp {}

impl AppCleanUp {
    pub fn fairing() -> Self {
        Self {}
    }
}

#[rocket::async_trait]
impl Fairing for AppCleanUp {
    fn info(&self) -> Info {
        Info {
            name: "app-clean-up",
            kind: Kind::Ignite | Kind::Liftoff,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<Build>) -> rocket::fairing::Result {
        let Some(config) = rocket.state::<Config>() else {
            log::error!("there must be a config");
            return Err(rocket);
        };

        if config.applications.back_up_policy.is_some() && config.database.is_none() {
            log::error!("To use the automated backup feature a database connection is required");
            return Err(rocket);
        }

        Ok(rocket)
    }

    async fn on_liftoff(&self, rocket: &Rocket<Orbit>) {
        let Some(config) = rocket.state::<Config>() else {
            return;
        };

        let Some(apps) = rocket.state::<Apps>() else {
            log::error!("Apps must be available");
            return;
        };

        let Some(queue) = rocket.state::<AppTaskQueueProducer>() else {
            log::error!("Queue must be available");
            return;
        };

        let Some(Some(repository)) = rocket.state::<Option<AppPostgresRepository>>() else {
            log::error!("Postgres database must be available");
            return;
        };

        if let Some(time_to_restore) = config
            .applications
            .back_up_policy
            .as_ref()
            .and_then(|back_up_policy| back_up_policy.time_to_restore)
        {
            let backup_clean_up = AutomatedStaleBackUpDetector {
                queue: queue.clone(),
                repository: repository.clone(),
                time_to_restore,
            };
            tokio::spawn(async move {
                backup_clean_up
                    .fetch_stale_backups_and_schedule_deletion()
                    .await
            });
        }

        let Some(clean_up_policy) = config
            .applications
            .back_up_policy
            .as_ref()
            .and_then(|back_up_policy| back_up_policy.clean_up_policy.as_ref())
        else {
            return;
        };

        log::info!("Automated backup is enabled.");

        let clean_up_detector =
            AutomatedBackUpDetector::new(clean_up_policy.clone(), apps.clone(), queue.clone());
        tokio::spawn(async move { clean_up_detector.run_automated_backup_endlessly().await });
    }
}

struct AutomatedBackUpDetector {
    clean_up_policy: ApplicationCleanUpPolicy,
    apps: Apps,
    queue: AppTaskQueueProducer,
    ran: AtomicBool,
}

impl AutomatedBackUpDetector {
    fn new(
        clean_up_policy: ApplicationCleanUpPolicy,
        apps: Apps,
        queue: AppTaskQueueProducer,
    ) -> Self {
        Self {
            clean_up_policy,
            apps,
            queue,
            ran: AtomicBool::new(false),
        }
    }

    async fn find_apps_for_automated_backup(&self) -> Vec<AppName> {
        let app_updates = self.apps.app_updates().await;
        let apps = app_updates.borrow().clone();

        let time_to_exists_before = Utc::now() - self.clean_up_policy.time_to_use;
        apps.into_iter()
            .filter(|(_, app)| {
                app.created_at
                    .is_none_or(|created_at| created_at <= time_to_exists_before)
            })
            .filter(|(app_name, _)| {
                !self
                    .clean_up_policy
                    .permanent_apps
                    .iter()
                    .any(|selector| selector.matches(app_name))
            })
            .map(|(app_name, _)| app_name)
            .collect::<Vec<_>>()
    }

    async fn run_automated_backup_endlessly(&self) {
        loop {
            if let Some(duration_to_sleep) = self.next_sleep_duration(Utc::now()) {
                tokio::time::sleep(duration_to_sleep).await;
            }

            self.run_automated_backup().await;

            self.ran.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    async fn run_automated_backup(&self) {
        log::debug!("Checking for stale applications");

        let app_names_to_be_considered_for_back_up = self.find_apps_for_automated_backup().await;

        match try_join!(
            async {
                self.apps
                    .fetch_traefik_router_names(app_names_to_be_considered_for_back_up)
                    .await
                    .map_err(anyhow::Error::from)
            },
            async { self.fetch_metrics().await },
        ) {
            Ok((app_names_and_router_names, metrics)) => {
                let (with_access, without_access) =
                    metrics.group_router_metrics(app_names_and_router_names);

                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "Following applications will be keept active: {apps}",
                        apps = with_access
                            .into_iter()
                            .map(|(app_name, rate)| format!("{app_name} (rate: {rate:.1})"))
                            .fold(String::new(), |a, b| a + ", " + &b)
                    )
                }

                for app_name in without_access {
                    log::debug!("{app_name} considered stale, will be backed up");

                    let infrastructure_payload = match self
                        .apps
                        .fetch_app_as_backup_based_infrastructure_payload(&app_name)
                        .await
                    {
                        Ok(Some(infrastructure_payload)) => infrastructure_payload,
                        Ok(None) => {
                            log::warn!(
                                    "No infrastructure paylod for app back-up for {app_name} available. Ignoring"
                                );
                            continue;
                        }
                        Err(err) => {
                            log::warn!(
                                    "Cannot fetch infrastructure paylod for app back-up for {app_name}: {err}"
                                );
                            continue;
                        }
                    };

                    if let Err(err) = self
                        .queue
                        .enqueue_backup_task(app_name.clone(), infrastructure_payload)
                        .await
                    {
                        log::error!("Failed to schedule application back-up of {app_name}: {err}");
                    }
                }
            }
            Err(err) => {
                log::error!("Cannot fetch data to determine which applications are stale: {err}");
            }
        };
    }

    fn next_sleep_duration(&self, datetime: DateTime<Utc>) -> Option<Duration> {
        let duration_to_sleep =  self.clean_up_policy.busy_hours.as_ref()
            .and_then(|busy_hours| {
                if log::log_enabled!(log::Level::Trace) {
                    log::trace!("Checking if {datetime} is within {busy_hours}");
                }
                Some((datetime, busy_hours.ending_of_busy_hours(datetime)?))
            })
            .map(|(now, (busy_hours_start, busy_hours_duration))| {
                let busy_hours_end = busy_hours_start + busy_hours_duration;
                let duration_to_sleep = (busy_hours_end - now)
                    .to_std()
                    .expect("This should be always greater zero");

                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "Currently in the busy hours, so I'm waiting until {busy_hours_end} (for {duration}, started at {busy_hours_start}) before automatically backing up some applications.",
                        duration = humantime::format_duration(duration_to_sleep)
                    );
                }

                duration_to_sleep
            });

        match duration_to_sleep {
            Some(duration_to_sleep) => Some(duration_to_sleep),
            None if self.ran.load(std::sync::atomic::Ordering::SeqCst) => {
                Some(if cfg!(debug_assertions) {
                    std::time::Duration::from_mins(1)
                } else {
                    log::trace!("Waiting for 10 minutes…");
                    std::time::Duration::from_mins(10)
                })
            }
            _ => None,
        }
    }

    async fn fetch_metrics(&self) -> Result<PrometheusQueryResponse> {
        match &self.clean_up_policy.metrics_provider {
            crate::config::RouterMetricsProvider::Prometheus {
                prometheus_url: url,
            } => {
                let mut query_url = url.join("/api/v1/query")?;
                query_url.set_query(Some(
                    &format!(
                        "query=max by (router) (increase(traefik_router_requests_total[{d}]))&time={now}",
                        d = humantime::format_duration(self.clean_up_policy.time_to_use).to_string()
                            // 2h 37m must be converted into 2h37m for the Prometheus API
                            .replace(" ", ""),
                        now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    ),
                ));

                let prometheus_query_response = reqwest::get(query_url)
                    .await?
                    .json::<PrometheusQueryResponse>()
                    .await?;

                if log::log_enabled!(log::Level::Trace) {
                    log::trace!("Prometheus increase query result: {prometheus_query_response}")
                }

                Ok(prometheus_query_response)
            }
        }
    }
}

#[derive(Debug)]
struct PrometheusQueryResponse {
    data: BTreeMap<String, f64>,
}

impl PrometheusQueryResponse {
    fn group_router_metrics(
        self,
        app_names_and_router_regex: HashMap<AppName, Vec<Regex>>,
    ) -> (Vec<(AppName, f64)>, Vec<AppName>) {
        let mut apps_with_recent_accesss = Vec::new();
        let mut apps_without_recent_accesss = Vec::new();

        for (app_name, router_regex) in app_names_and_router_regex.into_iter() {
            let mut sum = 0.0f64;

            for router_regex in router_regex {
                for (router_name, v) in &self.data {
                    if router_regex.is_match(router_name) {
                        sum += v;
                    }
                }
            }

            if sum > 0f64 {
                apps_with_recent_accesss.push((app_name, sum));
            } else {
                apps_without_recent_accesss.push(app_name);
            }
        }

        (apps_with_recent_accesss, apps_without_recent_accesss)
    }
}

impl std::fmt::Display for PrometheusQueryResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, (k, v)) in self.data.iter().enumerate() {
            if i > 0 {
                write!(f, ", {k} = {v}")?;
            } else {
                write!(f, "{k} = {v}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
impl PartialEq for PrometheusQueryResponse {
    fn eq(&self, other: &Self) -> bool {
        if self.data.len() != (other.data.len()) {
            return false;
        }

        for (k, v) in &self.data {
            let Some(other_v) = other.data.get(k) else {
                return false;
            };

            if (v - other_v).abs() > f64::EPSILON {
                return false;
            }
        }

        true
    }
}

impl<'de> serde::Deserialize<'de> for PrometheusQueryResponse {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use std::str::FromStr;
        let value = serde_json::Value::deserialize(deserializer)?;

        let data = value
            .as_object()
            .and_then(|v| v.get("data"))
            .and_then(|data| data.as_object())
            .and_then(|data| data.get("result"))
            .and_then(|result| result.as_array())
            .and_then(|metrics| {
                let mut data = BTreeMap::new();
                for metric in metrics {
                    let Some(router) = metric
                        .as_object()
                        .and_then(|metric| metric.get("metric"))
                        .and_then(|metric| metric.as_object())
                        .and_then(|metric| metric.get("router"))
                        .and_then(|router| router.as_str())
                    else {
                        continue;
                    };

                    let Some(value) = metric
                        .as_object()
                        .and_then(|metric| metric.get("value"))
                        .and_then(|value| value.as_array())
                        .map(|values| values.iter())
                        .and_then(|mut v_it| {
                            // The second value is the value that represents the query result
                            v_it.next();
                            v_it.next()
                        })
                        .and_then(|value| value.as_str())
                        .and_then(|value| f64::from_str(value).ok())
                    else {
                        continue;
                    };

                    data.insert(router.to_string(), value);
                }

                if !data.is_empty() {
                    Some(data)
                } else {
                    None
                }
            });

        data.map(|data| PrometheusQueryResponse { data })
            .ok_or_else(|| {
                serde::de::Error::custom(format!("Found no data to deserialize in {value}"))
            })
    }
}

struct AutomatedStaleBackUpDetector {
    queue: AppTaskQueueProducer,
    repository: AppPostgresRepository,
    time_to_restore: Duration,
}

impl AutomatedStaleBackUpDetector {
    async fn fetch_stale_backups_and_schedule_deletion(&self) {
        loop {
            let older_than = Utc::now() - self.time_to_restore;
            log::debug!("Searching for stale backups older than {older_than}.");

            match self.repository.fetch_backup_older_than(older_than).await {
                Ok(apps) => {
                    for (app_name, created_at) in apps {
                        if let Err(err) = self.queue.enqueue_delete_task(app_name.clone()).await {
                            log::error!("Cannot enqueue {app_name} (backup created at {created_at}) for deletion to clean up: {err}")
                        }
                    }
                }
                Err(err) => {
                    log::error!("Cannot fetch stale backups for clean up: {err}");
                }
            }

            tokio::time::sleep(std::time::Duration::from_mins(10)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infrastructure::Dummy, sc};
    use chrono::{TimeDelta, TimeZone};
    use std::str::FromStr;

    #[test]
    fn parse_prometheus_query_reponse() {
        let response = serde_json::json!({
          "status": "success",
          "data": {
            "resultType": "vector",
            "result": [
              {
                "metric": {
                  "router": "prevant@docker"
                },
                "value": [
                    11.11,
                  "11"
                ]
              },
              {
                "metric": {
                  "router": "a-blog@docker"
                },
                "value": [
                    22.22,
                  "22"
                ]
              },
              {
                "metric": {
                  "router": "b-blog@docker"
                },
                "value": [
                    33.33,
                  "33"
                ]
              }
            ],
            "stats": {
              "timings": {
                "evalTotalTime": 0.000113196,
                "resultSortTime": 0,
                "queryPreparationTime": 0.000015532,
                "innerEvalTime": 0.000085301,
                "execQueueTime": 0.00009662,
                "execTotalTime": 0.00012675
              },
              "samples": {
                "totalQueryableSamples": 45,
                "peakSamples": 14
              }
            }
          }
        });

        let data = serde_json::from_value::<PrometheusQueryResponse>(response).unwrap();

        assert_eq!(
            data,
            PrometheusQueryResponse {
                data: BTreeMap::from([
                    (String::from("a-blog@docker"), 22.),
                    (String::from("b-blog@docker"), 33.),
                    (String::from("prevant@docker"), 11.),
                ])
            }
        )
    }

    #[test]
    fn group_router_metrics() {
        let response = PrometheusQueryResponse {
            data: BTreeMap::from([
                (String::from("a-blog@docker"), 22.22),
                (String::from("b-blog@docker"), 0.0),
                (String::from("prevant@docker"), 11.11),
            ]),
        };

        let (with_access, without_access) = response.group_router_metrics(HashMap::from([
            (
                AppName::from_str("a-blog").unwrap(),
                vec![Regex::new("^a-.*@docker$").unwrap()],
            ),
            (
                AppName::from_str("b-blog").unwrap(),
                vec![Regex::new("^b-.*@docker$").unwrap()],
            ),
        ]));

        assert_eq!(
            with_access,
            vec![(AppName::from_str("a-blog").unwrap(), 22.22),]
        );
        assert_eq!(without_access, vec![AppName::from_str("b-blog").unwrap()]);
    }

    #[tokio::test]
    async fn find_app_names_for_automated_backup() {
        let config = crate::config_from_str!(
            r#"
            [applications.backups.automated]
            metricsProvider = { prometheusUrl  = "http://localhost:9090/" }
            "#
        );

        let now = Utc::now();

        let dummy_infra = Dummy::new()
            .with_app(
                AppName::master(),
                vec![sc!("nginx")],
                now - TimeDelta::weeks(100),
            )
            .with_app(
                AppName::from_str("to-be-kept-alive").unwrap(),
                vec![sc!("nginx")],
                now,
            )
            .with_app(
                AppName::from_str("to-be-backed-up").unwrap(),
                vec![sc!("nginx")],
                now - TimeDelta::days(2),
            )
            .with_app(
                AppName::from_str("to-be-kept-alive-2").unwrap(),
                vec![sc!("nginx")],
                now - TimeDelta::hours(2) + TimeDelta::minutes(1),
            );
        let apps = Apps::new(config.clone(), Box::new(dummy_infra)).unwrap();
        while apps.app_updates().await.borrow_and_update().is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let detector = AutomatedBackUpDetector {
            clean_up_policy: config
                .applications
                .back_up_policy
                .unwrap()
                .clean_up_policy
                .unwrap(),
            apps,
            queue: AppTaskQueueProducer::no_op(),
            ran: AtomicBool::new(true),
        };

        let app_names = detector.find_apps_for_automated_backup().await;

        assert_eq!(
            app_names,
            vec![AppName::from_str("to-be-backed-up").unwrap()]
        );
    }

    #[test]
    fn sleep_interval_with_busy_hours() {
        let config = crate::config_from_str!(
            r#"
            [applications.backups.automated]
            metricsProvider = { prometheusUrl  = "http://localhost:9090/" }
            busyHours = { start = "0 0 8 * * Mon-Fri", end = "0 0 16 * * Mon-Fri" }
            "#
        );
        let dummy_infra = Dummy::new();
        let apps = Apps::new(config.clone(), Box::new(dummy_infra)).unwrap();
        let detector = AutomatedBackUpDetector {
            clean_up_policy: config
                .applications
                .back_up_policy
                .unwrap()
                .clean_up_policy
                .unwrap(),
            apps,
            queue: AppTaskQueueProducer::no_op(),
            ran: AtomicBool::new(false),
        };

        let datetime = Utc
            .with_ymd_and_hms(2026, 2, 21, 15, 6, 0)
            .single()
            .unwrap();
        let sleep_duration = detector.next_sleep_duration(datetime);
        assert_eq!(sleep_duration, None);

        let datetime = Utc
            .with_ymd_and_hms(2026, 2, 20, 15, 6, 0)
            .single()
            .unwrap();
        let sleep_duration = detector.next_sleep_duration(datetime);
        assert_eq!(sleep_duration, Some(Duration::from_mins(1)));
    }

    #[test]
    fn sleep_interval_without_busy_hours() {
        let config = crate::config_from_str!(
            r#"
            [applications.backups.automated]
            metricsProvider = { prometheusUrl  = "http://localhost:9090/" }
            "#
        );
        let dummy_infra = Dummy::new();
        let apps = Apps::new(config.clone(), Box::new(dummy_infra)).unwrap();
        let detector = AutomatedBackUpDetector {
            clean_up_policy: config
                .applications
                .back_up_policy
                .unwrap()
                .clean_up_policy
                .unwrap(),
            apps,
            queue: AppTaskQueueProducer::no_op(),
            ran: AtomicBool::new(false),
        };

        let sleep_duration = detector.next_sleep_duration(Utc::now());
        assert_eq!(sleep_duration, None);

        detector
            .ran
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let sleep_duration = detector.next_sleep_duration(Utc::now());
        assert_eq!(sleep_duration, Some(Duration::from_mins(1)));
    }
}
