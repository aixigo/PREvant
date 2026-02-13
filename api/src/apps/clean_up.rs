use crate::{
    apps::{repository::AppPostgresRepository, AppTaskQueueProducer, Apps},
    config::{ApplicationCleanUpPolicy, Config},
    models::AppName,
};
use anyhow::Result;
use chrono::Utc;
use futures::try_join;
use rocket::{
    fairing::{Fairing, Info, Kind},
    Build, Orbit, Rocket,
};
use std::{
    collections::{BTreeMap, HashSet},
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
        tokio::spawn(async move {
            clean_up_detector
                .fetch_metrics_and_schedule_clean_up()
                .await
        });
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

    async fn fetch_metrics_and_schedule_clean_up(&self) {
        loop {
            self.sleep().await;

            log::debug!("Checking for stale applications");

            match try_join!(
                async {
                    // TODO: use app_updates to get services too
                    // also, make sure that self.apps.app_updates() returns a clone
                    self.apps
                        .fetch_app_names()
                        .await
                        .map_err(anyhow::Error::from)
                },
                async { self.fetch_metrics().await }
            ) {
                Ok((app_names, metrics)) => {
                    // TODO: filter the application name that should be kept
                    let stale_applications = metrics.retain_stale_app_names(app_names);

                    for app_name in stale_applications {
                        log::debug!("{app_name} considered stale, will be backed up");

                        let Some(infrastructure_payload) = self
                            .apps
                            .fetch_app_as_backup_based_infrastructure_payload(&app_name)
                            .await
                            .unwrap()
                        else {
                            log::warn!(
                                "Cannot fetch infrastructure paylod for app back-up for {app_name}"
                            );
                            continue;
                        };

                        // TODO filter by creation time.
                        if let Err(err) = self
                            .queue
                            .enqueue_backup_task(app_name.clone(), infrastructure_payload)
                            .await
                        {
                            log::error!(
                                "Failed to schedule application back-up of {app_name}: {err}"
                            );
                        }
                    }
                }
                Err(err) => {
                    log::error!(
                        "Cannot fetch data to determine which applications are stale: {err}"
                    );
                }
            };

            self.ran.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    async fn sleep(&self) {
        let duration_to_sleep = if let Some(busy_hours) = self.clean_up_policy.busy_hours.as_ref() {
            let now = Utc::now();
            if let Some((busy_hours_start, busy_hours_duration)) =
                busy_hours.ending_of_busy_hours(now)
            {
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

                Some(duration_to_sleep)
            } else {
                None
            }
        } else if self.ran.load(std::sync::atomic::Ordering::Relaxed) {
            Some(std::time::Duration::from_mins(10))
        } else {
            None
        };

        if let Some(duration_to_sleep) = duration_to_sleep {
            tokio::time::sleep(duration_to_sleep).await;
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

                if log::log_enabled!(log::Level::Debug) {
                    log::debug!("Prometheus increase query result: {prometheus_query_response}")
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
    fn retain_stale_app_names(&self, mut app_names: HashSet<AppName>) -> HashSet<AppName> {
        app_names.retain(|app_name| {
            let mut sum: Option<f64> = None;

            for (k, v) in &self.data {
                if k.starts_with(app_name.as_str()) {
                    sum = Some(sum.map(|n| n + *v).unwrap_or(*v));
                }
            }

            sum.is_some_and(|n| n.abs() <= f64::EPSILON)
        });
        app_names
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
    fn retain_stale_app_names() {
        let response = PrometheusQueryResponse {
            data: BTreeMap::from([
                (String::from("a-blog@docker"), 22.22),
                (String::from("b-blog@docker"), 0.0),
                (String::from("prevant@docker"), 11.11),
            ]),
        };

        assert_eq!(
            response.retain_stale_app_names(HashSet::from([
                AppName::from_str("a").unwrap(),
                AppName::from_str("b").unwrap(),
            ])),
            HashSet::from([AppName::from_str("b").unwrap()])
        );

        assert_eq!(
            response.retain_stale_app_names(HashSet::from([AppName::from_str("c").unwrap(),])),
            HashSet::new(),
            // TODO: what if an application hasn't been accessed
            "An application name that is not in the set shouldn't be considered stale if an application hasn't been started there are no metrics available"
        );
    }
}
