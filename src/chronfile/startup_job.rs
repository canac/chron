use super::RetryConfig;
use crate::chron_service::StartupJobOptions;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StartupJob {
    pub command: String,
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    retry: RetryConfig,
}

impl StartupJob {
    pub fn get_options(&self) -> StartupJobOptions {
        StartupJobOptions { retry: self.retry }
    }
}

#[cfg(test)]
mod tests {
    use super::super::RetryLimit;
    use anyhow::Result;
    use std::time::Duration;
    use toml::de::Error;

    use super::*;

    /// Parse a startup job and return its keep alive config
    fn parse_retry(input: &'static str) -> std::result::Result<RetryConfig, Error> {
        Ok(toml::from_str::<StartupJob>(input)?.get_options().retry)
    }

    #[test]
    fn test_retry_omitted() -> Result<()> {
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
    fn test_retry() -> Result<()> {
        assert_eq!(
            parse_retry("command = 'echo'\nretry = { limit = 3, delay = '10m' }")?,
            RetryConfig {
                limit: RetryLimit::Limited(3),
                delay: Some(Duration::from_secs(600)),
            },
        );
        Ok(())
    }
}
