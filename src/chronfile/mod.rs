mod job;
mod retry_config;

pub use self::job::Job;
pub use self::retry_config::{RetryConfig, RetryLimit};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};
use tokio::fs::read_to_string;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub shell: Option<String>,
    #[serde(rename = "onError")]
    pub on_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chronfile {
    #[serde(default)]
    pub config: Config,

    #[serde(default)]
    pub jobs: HashMap<String, Job>,
}

impl Chronfile {
    /// Load a chronfile by its path
    pub async fn load(path: &PathBuf) -> Result<Self> {
        let toml_str = read_to_string(path)
            .await
            .with_context(|| format!("Failed to read chronfile {}", path.display()))?;
        toml::from_str(&toml_str)
            .with_context(|| format!("Failed to deserialize TOML chronfile {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use self::job::Job;
    use assert_matches::assert_matches;

    use super::*;

    fn load_chronfile(toml: &str) -> Result<Chronfile> {
        Ok(toml::from_str(toml)?)
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
        assert_matches!(jobs.get("startup"), Some(Job { schedule, command, working_dir, disabled, .. }) => {
            assert_eq!(schedule, &None);
            assert_eq!(command, &"echo");
            assert_eq!(working_dir, &Some(PathBuf::from("/directory")));
            assert_eq!(disabled, &false);
        });
        assert_matches!(jobs.get("scheduled"), Some(Job { schedule, working_dir, command, disabled, .. }) => {
            assert_eq!(schedule, &Some("* * * * * *".to_owned()));
            assert_eq!(command, &"echo");
            assert_eq!(working_dir, &None);
            assert_eq!(disabled, &false);
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

        assert!(!jobs.get("startup").unwrap().disabled);
        assert!(jobs.get("scheduled").unwrap().disabled);
        Ok(())
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
        assert_eq!(chronfile.config.shell.as_deref(), Some("bash"));
        assert_eq!(chronfile.config.on_error.as_deref(), Some("echo failed"));

        let chronfile = load_chronfile("[config]")?;
        assert_eq!(chronfile.config.shell, None);
        assert_eq!(chronfile.config.on_error, None);

        let chronfile = load_chronfile("")?;
        assert_eq!(chronfile.config.shell, None);
        assert_eq!(chronfile.config.on_error, None);

        Ok(())
    }
}
