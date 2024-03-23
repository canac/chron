use crate::chron_service::{self, MakeUpMissedRuns, ScheduledJobOptions};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

fn default_make_up_missed_runs() -> MakeUpMissedRuns {
    MakeUpMissedRuns::Limited(0)
}

// Allow MakeUpMissedRuns to be deserialized from a boolean or an integer
fn deserialize_make_up_missed_runs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<MakeUpMissedRuns, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields, untagged)]
    enum MakeUpRunsVariant {
        Simple(bool),
        Complex(usize),
    }

    Ok(match MakeUpRunsVariant::deserialize(deserializer)? {
        MakeUpRunsVariant::Simple(false) => MakeUpMissedRuns::Limited(0),
        MakeUpRunsVariant::Simple(true) => MakeUpMissedRuns::Unlimited,
        MakeUpRunsVariant::Complex(limit) => MakeUpMissedRuns::Limited(limit),
    })
}

// Allow RetryConfig to be deserialized from a boolean or a full retry config
fn deserialize_retry_config<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<RetryConfig, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields, untagged)]
    enum RetryConfigVariant {
        Simple(bool),
        Complex {
            limit: Option<usize>,
            #[serde(default, with = "humantime_serde")]
            delay: Option<Duration>,
        },
    }

    Ok(match RetryConfigVariant::deserialize(deserializer)? {
        RetryConfigVariant::Simple(retry) => RetryConfig {
            retry,
            ..Default::default()
        },
        RetryConfigVariant::Complex { limit, delay } => RetryConfig {
            retry: true,
            limit,
            delay,
        },
    })
}

#[derive(Debug, Default, Deserialize, Eq, PartialEq)]
struct RetryConfig {
    retry: bool,
    limit: Option<usize>,
    delay: Option<Duration>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScheduledJob {
    pub schedule: String,
    pub command: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(
        default = "default_make_up_missed_runs",
        deserialize_with = "deserialize_make_up_missed_runs"
    )]
    make_up_missed_runs: MakeUpMissedRuns,
    #[serde(default, deserialize_with = "deserialize_retry_config")]
    retry: RetryConfig,
}

impl ScheduledJob {
    pub fn get_options(&self) -> ScheduledJobOptions {
        ScheduledJobOptions {
            make_up_missed_runs: self.make_up_missed_runs,
            retry: chron_service::RetryConfig {
                failures: self.retry.retry,
                successes: false,
                limit: self.retry.limit,
                delay: self.retry.delay,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_makeup_missed_runs() -> Result<()> {
        let make_up_missed_runs: MakeUpMissedRuns = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRuns = false",
        )?
        .make_up_missed_runs;
        assert_eq!(make_up_missed_runs, MakeUpMissedRuns::Limited(0));

        let make_up_missed_runs: MakeUpMissedRuns = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRuns = true",
        )?
        .make_up_missed_runs;
        assert_eq!(make_up_missed_runs, MakeUpMissedRuns::Unlimited);

        let make_up_missed_runs: MakeUpMissedRuns = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRuns = 3",
        )?
        .make_up_missed_runs;
        assert_eq!(make_up_missed_runs, MakeUpMissedRuns::Limited(3));

        Ok(())
    }

    #[test]
    fn test_retry() -> Result<()> {
        assert!(
            !toml::from_str::<ScheduledJob>(
                "command = 'echo'\nschedule = '* * * * * *'\nretry = false"
            )?
            .retry
            .retry
        );

        assert!(
            toml::from_str::<ScheduledJob>(
                "command = 'echo'\nschedule = '* * * * * *'\nretry = true"
            )?
            .retry
            .retry
        );

        assert_eq!(
            toml::from_str::<ScheduledJob>(
                "command = 'echo'\nschedule = '* * * * * *'\nretry = { delay = '10m' }"
            )?
            .retry,
            RetryConfig {
                retry: true,
                limit: None,
                delay: Some(Duration::from_secs(600)),
            }
        );

        assert!(
            toml::from_str::<ScheduledJob>(
                "command = 'echo'\nschedule = '* * * * * *'\nretry = { failures = true, delay = '10m' }"
            ).is_err()
        );

        Ok(())
    }
}
