use crate::models::AppName;
use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::Deserialize;
use std::{str::FromStr, time::Duration};
use url::Url;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Applications {
    pub max: Option<usize>,
    #[serde(default = "AppName::master")]
    pub default_app: AppName,
    #[serde(default)]
    pub replication_condition: ReplicateApplicationCondition,
    #[serde(default, rename = "backups")]
    pub back_up_policy: Option<ApplicationBackUpPolicy>,
}

impl Default for Applications {
    fn default() -> Self {
        Self {
            max: None,
            default_app: AppName::master(),
            replication_condition: Default::default(),
            back_up_policy: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub enum ReplicateApplicationCondition {
    #[default]
    #[serde(rename = "always-from-default-app")]
    AlwaysFromDefaultApp,
    #[serde(rename = "replicate-only-when-requested")]
    ExplicitlyMentioned,
    #[serde(rename = "never")]
    Never,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationBackUpPolicy {
    #[serde(
        default = "two_weeks_to_retore",
        deserialize_with = "unlimited_duration"
    )]
    pub time_to_restore: Option<Duration>,
    #[serde(rename = "automated")]
    pub clean_up_policy: Option<ApplicationCleanUpPolicy>,
}

fn unlimited_duration<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = String::deserialize(deserializer)?;
    if v == "unlimited" {
        Ok(None)
    } else {
        Ok(Some(
            humantime::parse_duration(&v).map_err(serde::de::Error::custom)?,
        ))
    }
}

const fn two_weeks_to_retore() -> Option<Duration> {
    Some(Duration::from_hours(24 * 14))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationCleanUpPolicy {
    #[serde(
        default = "two_hours_to_use",
        deserialize_with = "deserialize_from_humantime"
    )]
    pub time_to_use: Duration,
    pub metrics_provider: RouterMetricsProvider,

    #[serde(default)]
    pub busy_hours: Option<BusyHours>,
}

fn deserialize_from_humantime<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = String::deserialize(deserializer)?;
    humantime::parse_duration(&v).map_err(serde::de::Error::custom)
}

const fn two_hours_to_use() -> Duration {
    Duration::from_hours(2)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(untagged)]
pub enum RouterMetricsProvider {
    #[serde(rename_all = "camelCase")]
    Prometheus { prometheus_url: Url },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BusyHours {
    #[serde(deserialize_with = "deserialize_from_cron_schedule")]
    pub start: Schedule,
    #[serde(deserialize_with = "deserialize_from_cron_schedule")]
    pub end: Schedule,
}

impl BusyHours {
    pub fn ending_of_busy_hours(
        &self,
        datetime: DateTime<Utc>,
    ) -> Option<(DateTime<Utc>, chrono::Duration)> {
        let previovs_start = self
            .start
            .upcoming(datetime.timezone())
            .next_back()
            .expect("Cron expression may never end");
        let next_end = self
            .end
            .upcoming(datetime.timezone())
            .next()
            .expect("Cron expression may never end");

        if previovs_start <= datetime && datetime <= next_end {
            return Some((previovs_start, next_end - previovs_start));
        }

        None
    }
}

impl Default for BusyHours {
    fn default() -> Self {
        Self {
            start: Schedule::from_str("0 0 8 * * Mon-Fri").unwrap(),
            end: Schedule::from_str("0 0 16 * * Mon-Fri").unwrap(),
        }
    }
}

fn deserialize_from_cron_schedule<'de, D>(deserializer: D) -> Result<Schedule, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = String::deserialize(deserializer)?;
    Schedule::from_str(&v).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_from_str;

    #[test]
    fn parse_without_application() {
        let config = config_from_str!("");

        assert_eq!(
            config.applications,
            Applications {
                max: None,
                default_app: AppName::master(),
                replication_condition: ReplicateApplicationCondition::AlwaysFromDefaultApp,
                back_up_policy: None,
            }
        )
    }

    #[test]
    fn parse_one_week_time_to_restore() {
        let config = config_from_str!(
            r#"
            [applications.backups]
            timeToRestore = '1week'
            "#
        );

        assert_eq!(
            config.applications.back_up_policy.unwrap().time_to_restore,
            Some(Duration::from_hours(24 * 7))
        );
    }

    #[test]
    fn parse_unlimited_time_to_restore() {
        let config = config_from_str!(
            r#"
            [applications.backups]
            timeToRestore = 'unlimited'
            "#
        );

        assert_eq!(
            config.applications.back_up_policy.unwrap().time_to_restore,
            None
        );
    }

    #[test]
    fn parse_prometheus_based_backup() {
        let config = config_from_str!(
            r#"
            [applications.backups.automated]
            metricsProvider = { prometheusUrl  = "http://localhost:9090/" }
            "#
        );

        assert_eq!(
            config.applications,
            Applications {
                max: None,
                default_app: AppName::master(),
                replication_condition: ReplicateApplicationCondition::AlwaysFromDefaultApp,
                back_up_policy: Some(ApplicationBackUpPolicy {
                    clean_up_policy: Some(ApplicationCleanUpPolicy {
                        time_to_use: Duration::from_hours(2),
                        metrics_provider: RouterMetricsProvider::Prometheus {
                            prometheus_url: Url::parse("http://localhost:9090/").unwrap()
                        },
                        busy_hours: None,
                    }),
                    time_to_restore: Some(Duration::from_hours(24 * 14)),
                }),
            }
        );
    }

    #[test]
    fn parse_prometheus_based_backup_with_custom_time_to_use() {
        let config = config_from_str!(
            r#"
            [applications.backups.automated]
            timeToUse = '20min'
            metricsProvider = { prometheusUrl = "http://localhost:9090/" }
            "#
        );

        let clean_up_policy = config
            .applications
            .back_up_policy
            .unwrap()
            .clean_up_policy
            .unwrap();

        assert_eq!(clean_up_policy.time_to_use, Duration::from_mins(20));
    }

    #[test]
    fn parse_automated_backup_with_busy_hours() {
        let config = config_from_str!(
            r#"
            [applications.backups.automated]
            metricsProvider = { prometheusUrl  = "http://localhost:9090/" }
            busyHours = { start = "0 0 8 * * Mon-Fri", end = "0 0 16 * * Mon-Fri" }
            "#
        );

        let clean_up_policy = config
            .applications
            .back_up_policy
            .unwrap()
            .clean_up_policy
            .unwrap();

        assert_eq!(clean_up_policy.busy_hours, Some(BusyHours::default()))
    }

    mod busy_hours {
        use super::*;
        use chrono::TimeZone;

        #[test]
        fn datetime_within_busy_hours() {
            let busy_hours = BusyHours::default();

            let now = Utc
                .with_ymd_and_hms(2026, 2, 13, 11, 22, 33)
                .single()
                .unwrap();

            assert_eq!(
                busy_hours.ending_of_busy_hours(now),
                Some((
                    Utc.with_ymd_and_hms(2026, 2, 13, 8, 00, 00)
                        .single()
                        .unwrap(),
                    chrono::Duration::hours(8)
                ))
            )
        }

        #[rstest::rstest]
        #[case::after_working_hours(
            Utc.with_ymd_and_hms(2026, 2, 13, 21, 22, 33).single().unwrap()
        )]
        #[case::before_working_hours(
            Utc.with_ymd_and_hms(2026, 2, 13, 7, 22, 33).single().unwrap()
        )]
        fn datetime_outside_busy_hours(#[case] simulated_now: DateTime<Utc>) {
            let busy_hours = BusyHours::default();

            assert_eq!(busy_hours.ending_of_busy_hours(simulated_now), None,)
        }
    }
}
