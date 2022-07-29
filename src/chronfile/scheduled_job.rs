use crate::chron_service::{self, MakeUpMissedRuns, ScheduledJobOptions};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
enum MakeUpRunsVariant {
    Simple(bool),
    Complex(usize),
}

impl Default for MakeUpRunsVariant {
    fn default() -> Self {
        MakeUpRunsVariant::Simple(false)
    }
}

impl From<MakeUpRunsVariant> for MakeUpMissedRuns {
    fn from(val: MakeUpRunsVariant) -> Self {
        match val {
            MakeUpRunsVariant::Simple(false) => MakeUpMissedRuns::Limited(0),
            MakeUpRunsVariant::Simple(true) => MakeUpMissedRuns::Unlimited,
            MakeUpRunsVariant::Complex(limit) => MakeUpMissedRuns::Limited(limit),
        }
    }
}

// Allow RetryConfig to be deserialized from a boolean or a full retry config
#[derive(Deserialize)]
#[serde(deny_unknown_fields, untagged)]
enum RetryVariant {
    Simple(bool),
    Complex {
        limit: Option<usize>,
        #[serde(default, with = "humantime_serde")]
        delay: Option<Duration>,
    },
}

#[derive(Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(from = "RetryVariant")]
struct RetryConfig {
    retry: bool,
    limit: Option<usize>,
    delay: Option<Duration>,
}

impl From<RetryVariant> for RetryConfig {
    fn from(value: RetryVariant) -> Self {
        match value {
            RetryVariant::Simple(retry) => RetryConfig {
                retry,
                ..Default::default()
            },
            RetryVariant::Complex { limit, delay } => RetryConfig {
                retry: true,
                limit,
                delay,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ScheduledJob {
    pub(super) schedule: String,
    pub(super) command: String,
    #[serde(default)]
    pub(super) disabled: bool,
    #[serde(default)]
    make_up_missed_runs: MakeUpRunsVariant,
    #[serde(default)]
    retry: RetryConfig,
}

impl ScheduledJob {
    pub fn get_options(&self) -> ScheduledJobOptions {
        ScheduledJobOptions {
            make_up_missed_runs: self.make_up_missed_runs.clone().into(),
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
        .make_up_missed_runs
        .into();
        assert_eq!(make_up_missed_runs, MakeUpMissedRuns::Limited(0));

        let make_up_missed_runs: MakeUpMissedRuns = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRuns = true",
        )?
        .make_up_missed_runs
        .into();
        assert_eq!(make_up_missed_runs, MakeUpMissedRuns::Unlimited);

        let make_up_missed_runs: MakeUpMissedRuns = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRuns = 3",
        )?
        .make_up_missed_runs
        .into();
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
