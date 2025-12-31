use super::RetryConfig;
use super::expand_tilde::expand_tilde;
use crate::chronfile::env::Env;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RawJobDefinition {
    pub schedule: Option<String>,
    pub command: String,
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub retry: RetryConfig,
}

#[derive(Debug, Eq, PartialEq)]
pub struct JobDefinition {
    pub schedule: Option<String>,
    pub command: String,
    pub working_dir: Option<PathBuf>,
    pub retry: RetryConfig,
}

impl JobDefinition {
    pub(super) fn from_raw(raw: RawJobDefinition, env: &Env) -> Self {
        Self {
            schedule: raw.schedule,
            command: raw.command,
            working_dir: raw.working_dir.map(|path| expand_tilde(&path, env)),
            retry: raw.retry,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::RetryLimit;
    use anyhow::Result;
    use std::time::Duration;

    use super::*;

    fn parse_job(input: &'static str) -> Result<JobDefinition> {
        Ok(JobDefinition::from_raw(
            toml::from_str(input)?,
            &Env::mock(),
        ))
    }

    /// Parse a job and return its retry config
    fn parse_retry(input: &'static str) -> Result<RetryConfig> {
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
        let job = parse_job("command = 'echo'\nworkingDir = '~/project'")?;
        assert_eq!(job.working_dir, Some(PathBuf::from("/Users/user/project")));
        Ok(())
    }
}
