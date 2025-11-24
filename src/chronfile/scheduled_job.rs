use super::RetryConfig;
use crate::chron_service::ScheduledJobOptions;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScheduledJob {
    pub schedule: String,
    pub command: String,
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    make_up_missed_run: bool,
    #[serde(default)]
    retry: RetryConfig,
}

impl ScheduledJob {
    pub fn get_options(&self) -> ScheduledJobOptions {
        ScheduledJobOptions {
            make_up_missed_run: self.make_up_missed_run,
            retry: self.retry,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::RetryLimit;
    use anyhow::Result;
    use std::time::Duration;
    use toml::de::Error;

    use super::*;

    /// Parse a scheduled job and return its retry config
    fn parse_retry(input: &'static str) -> std::result::Result<RetryConfig, Error> {
        Ok(toml::from_str::<ScheduledJob>(input)?.get_options().retry)
    }

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
    fn test_retry_omitted() -> Result<()> {
        assert_eq!(
            parse_retry("command = 'echo'\nschedule = '* * * * * *'")?,
            RetryConfig {
                limit: RetryLimit::Limited(0),
                delay: None,
            },
        );
        Ok(())
    }

    #[test]
    fn test_retry() -> Result<()> {
        assert_eq!(
            parse_retry(
                "command = 'echo'\nschedule = '* * * * * *'\nretry = { limit = 3, delay = '10m' }"
            )?,
            RetryConfig {
                limit: RetryLimit::Limited(3),
                delay: Some(Duration::from_secs(600)),
            },
        );
        Ok(())
    }
}
