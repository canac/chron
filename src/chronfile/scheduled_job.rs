use crate::chron_service::{self, ScheduledJobOptions};
use serde::{Deserialize, Deserializer};
use std::time::Duration;

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
    #[serde(default)]
    make_up_missed_run: bool,
    #[serde(default, deserialize_with = "deserialize_retry_config")]
    retry: RetryConfig,
}

impl ScheduledJob {
    pub fn get_options(&self) -> ScheduledJobOptions {
        ScheduledJobOptions {
            make_up_missed_run: self.make_up_missed_run,
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
    fn test_makeup_missed_run() -> Result<()> {
        let scheduled_job = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRun = false",
        )?;
        assert!(!scheduled_job.make_up_missed_run);

        let scheduled_job = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRun = true",
        )?;
        assert!(scheduled_job.make_up_missed_run);

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
