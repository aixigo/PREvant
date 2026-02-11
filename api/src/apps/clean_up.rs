use crate::{
    apps::{AppTaskQueueProducer, Apps},
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
    sync::Mutex,
};

pub struct AppCleanUp {
    clean_up_detector: Mutex<Option<CleanUpDetector>>,
}

impl AppCleanUp {
    pub fn fairing() -> Self {
        Self {
            clean_up_detector: Mutex::new(None),
        }
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
            return Err(rocket);
        };

        if let Some(clean_up_policy) = &config.applications.back_up_policy {
            let Some(apps) = rocket.state::<Apps>() else {
                log::error!("Apps must be available");
                return Err(rocket);
            };

            let Some(queue) = rocket.state::<AppTaskQueueProducer>() else {
                log::error!("Queue must be available");
                return Err(rocket);
            };

            let mut clean_up_detector = self.clean_up_detector.lock().unwrap();
            *clean_up_detector = Some(CleanUpDetector {
                clean_up_policy: clean_up_policy.clone(),
                apps: apps.clone(),
                queue: queue.clone(),
            });
        }

        Ok(rocket)
    }

    async fn on_liftoff(&self, _rocket: &Rocket<Orbit>) {
        let mut clean_up_detector = self.clean_up_detector.lock().unwrap();
        if let Some(clean_up_detector) = clean_up_detector.take() {
            tokio::task::spawn(async move {
                clean_up_detector
                    .fetch_metrics_and_schedule_clean_up()
                    .await
            });
        }
    }
}

struct CleanUpDetector {
    clean_up_policy: ApplicationCleanUpPolicy,
    apps: Apps,
    queue: AppTaskQueueProducer,
}

impl CleanUpDetector {
    async fn fetch_metrics_and_schedule_clean_up(&self) {
        loop {
            log::debug!("Checking for stale applications");

            match try_join!(
                async {
                    self.apps
                        .fetch_app_names()
                        .await
                        .map_err(anyhow::Error::from)
                },
                async { self.fetch_metrics().await }
            ) {
                Ok((app_names, metrics)) => {
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

            tokio::time::sleep(std::time::Duration::from_mins(10)).await;
        }
    }

    async fn fetch_metrics(&self) -> Result<PrometheusQueryResponse> {
        match &self.clean_up_policy.metrics_provider {
            crate::config::RouterMetricsProvider::Prometheus { url } => {
                let mut query_url = url.join("/api/v1/query")?;
                query_url.set_query(Some(
                    &format!(
                        // TODO: make time interval configurable
                        "query=max by (router) (increase(traefik_router_requests_total[5m]))&time={now}",
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
