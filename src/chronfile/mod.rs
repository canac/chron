mod config;
pub mod env;
mod expand_tilde;
mod job_definition;
mod retry_config;

pub use self::job_definition::JobDefinition;
use self::job_definition::RawJobDefinition;
pub use self::retry_config::{RetryConfig, RetryLimit};
pub use crate::chronfile::config::Config;
use crate::chronfile::{config::RawConfig, env::Env};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};
use tokio::fs::read_to_string;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChronfile {
    #[serde(default)]
    config: RawConfig,

    #[serde(default)]
    jobs: HashMap<String, RawJobDefinition>,
}

#[derive(Debug)]
pub struct Chronfile {
    pub config: Config,
    pub jobs: HashMap<String, JobDefinition>,
}

impl Chronfile {
    fn from_raw(raw: RawChronfile, env: &Env) -> Result<Self> {
        let jobs = raw
            .jobs
            .into_iter()
            .filter(|(_, job)| !job.disabled)
            .map(|(name, job)| {
                validate_name(&name)?;
                Ok((name, JobDefinition::from_raw(job, env)))
            })
            .collect::<Result<_>>()?;

        Ok(Self {
            config: Config {
                shell: raw.config.shell.unwrap_or_else(|| env.shell.clone()),
                on_error: raw.config.on_error,
            },
            jobs,
        })
    }
}

impl Chronfile {
    /// Load a chronfile by its path
    pub async fn load(path: &PathBuf, env: &Env) -> Result<Self> {
        let toml_str = read_to_string(path)
            .await
            .with_context(|| format!("Failed to read chronfile {}", path.display()))?;
        let raw: RawChronfile = toml::from_str(&toml_str)
            .with_context(|| format!("Failed to deserialize TOML chronfile {}", path.display()))?;
        Self::from_raw(raw, env)
    }
}

/// Validate a job name
fn validate_name(name: &str) -> Result<()> {
    if name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || name
            .chars()
            .any(|char| !char.is_ascii_alphanumeric() && char != '-')
    {
        bail!("Invalid job name {name}")
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    fn load_chronfile(toml: &str) -> Result<Chronfile> {
        Chronfile::from_raw(toml::from_str(toml)?, &Env::mock())
    }

    #[test]
    fn test_empty() {
        assert!(load_chronfile("").is_ok());
    }

    #[test]
    fn test_simple() -> Result<()> {
        let jobs = load_chronfile(
            "[jobs.startup]
command = 'echo'
workingDir = '/directory'

            [jobs.scheduled]
schedule = '* * * * * *'
command = 'echo'",
        )?
        .jobs;

        assert_eq!(jobs.len(), 2);
        assert_matches!(jobs.get("startup"), Some(JobDefinition { schedule, command, working_dir, .. }) => {
            assert_eq!(schedule, &None);
            assert_eq!(command, &"echo");
            assert_eq!(working_dir, &Some(PathBuf::from("/directory")));
        });
        assert_matches!(jobs.get("scheduled"), Some(JobDefinition { schedule, working_dir, command, .. }) => {
            assert_eq!(schedule, &Some("* * * * * *".to_owned()));
            assert_eq!(command, &"echo");
            assert_eq!(working_dir, &None);
        });
        Ok(())
    }

    #[test]
    fn test_disabled() -> Result<()> {
        let jobs = load_chronfile(
            "[jobs.startup]
command = 'echo'
disabled = false

            [jobs.scheduled]
schedule = '* * * * * *'
command = 'echo'
disabled = true",
        )?
        .jobs;

        assert!(jobs.contains_key("startup"));
        assert!(!jobs.contains_key("scheduled"));
        Ok(())
    }

    #[test]
    fn test_invalid_name() {
        let error = load_chronfile(
            "[jobs.invalid--name]
command = 'echo'",
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Invalid job name invalid--name");
    }

    #[test]
    fn test_extra_fields() {
        assert!(load_chronfile("foo = 'bar'").is_err());

        assert!(
            load_chronfile(
                "[jobs.startup]
command = 'echo'
foo = 'bar'"
            )
            .is_err(),
        );

        assert!(
            load_chronfile(
                "[jobs.scheduled]
schedule = '* * * * * *'
command = 'echo'
foo = 'bar'"
            )
            .is_err(),
        );

        assert!(
            load_chronfile(
                "[jobs.startup]
command = 'echo'
retry = { foo = 'bar' }"
            )
            .is_err(),
        );

        assert!(
            load_chronfile(
                "[jobs.scheduled]
schedule = '* * * * * *'
command = 'echo'
retry = { foo = 'bar' }"
            )
            .is_err(),
        );
    }

    #[test]
    fn test_config() -> Result<()> {
        let chronfile = load_chronfile(
            "[config]
shell = 'bash'
onError = 'echo failed'",
        )?;
        assert_eq!(chronfile.config.shell, "bash");
        assert_eq!(chronfile.config.on_error.as_deref(), Some("echo failed"));

        let chronfile = load_chronfile("[config]")?;
        assert_eq!(chronfile.config.shell, "shell");
        assert_eq!(chronfile.config.on_error, None);

        let chronfile = load_chronfile("")?;
        assert_eq!(chronfile.config.shell, "shell");
        assert_eq!(chronfile.config.on_error, None);

        Ok(())
    }

    #[test]
    fn test_validate_name() {
        assert!(validate_name("abc").is_ok());
        assert!(validate_name("abc-def-ghi").is_ok());
        assert!(validate_name("123-456-789").is_ok());
        assert!(validate_name("-abc-def-ghi").is_err());
        assert!(validate_name("abc-def-ghi-").is_err());
        assert!(validate_name("abc--def-ghi").is_err());
        assert!(validate_name("1*2$3").is_err());
    }
}
