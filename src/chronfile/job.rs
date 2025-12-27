use super::RetryConfig;
use super::expand_tilde::expand_tilde;
use serde::{Deserialize, Deserializer};
use std::path::PathBuf;

fn deserialize_directory<'de, D>(deserializer: D) -> Result<Option<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let path = Option::deserialize(deserializer)?;
    Ok(path.map(|path| expand_tilde(&path)))
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Job {
    pub schedule: Option<String>,
    pub command: String,
    #[serde(default, deserialize_with = "deserialize_directory")]
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

    fn parse_job(input: &'static str) -> std::result::Result<Job, Error> {
        toml::from_str::<Job>(input)
    }

    /// Parse a job and return its retry config
    fn parse_retry(input: &'static str) -> std::result::Result<RetryConfig, Error> {
        Ok(parse_job(input)?.retry)
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

    #[test]
    fn test_working_dir() -> Result<()> {
        assert_eq!(
            parse_job("command = 'echo'\nworkingDir = '~/project'")?.working_dir,
            Some(dirs::home_dir().unwrap().join("project"))
        );
        Ok(())
    }
}
