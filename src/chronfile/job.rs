use super::RetryConfig;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Job {
    pub schedule: Option<String>,
    pub command: String,
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub retry: RetryConfig,
}

#[cfg(test)]
mod tests {
    use super::super::RetryLimit;
    use anyhow::Result;
    use std::time::Duration;
    use toml::de::Error;

    use super::*;

    /// Parse a job and return its retry config
    fn parse_retry(input: &'static str) -> std::result::Result<RetryConfig, Error> {
        Ok(toml::from_str::<Job>(input)?.retry)
    }

    #[test]
    fn test_startup_job_retry_omitted() -> Result<()> {
        assert_eq!(
            parse_retry("command = 'echo'")?,
            RetryConfig {
                limit: RetryLimit::Limited(0),
                delay: None,
            },
        );
        Ok(())
    }

    #[test]
    fn test_startup_job_retry() -> Result<()> {
        assert_eq!(
            parse_retry("command = 'echo'\nretry = { limit = 3, delay = '10m' }")?,
            RetryConfig {
                limit: RetryLimit::Limited(3),
                delay: Some(Duration::from_secs(600)),
            },
        );
        Ok(())
    }

    #[test]
    fn test_scheduled_job_retry_omitted() -> Result<()> {
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
    fn test_scheduled_job_retry() -> Result<()> {
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
