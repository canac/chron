mod retry_config;
mod scheduled_job;
mod startup_job;

pub use self::retry_config::{RetryConfig, RetryLimit};
pub use self::scheduled_job::ScheduledJob;
pub use self::startup_job::StartupJob;
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
    #[serde(rename = "startup", default)]
    pub startup_jobs: HashMap<String, StartupJob>,
    #[serde(rename = "scheduled", default)]
    pub scheduled_jobs: HashMap<String, ScheduledJob>,
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
    use self::scheduled_job::ScheduledJob;
    use self::startup_job::StartupJob;
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
        let chronfile = load_chronfile(
            "[startup.startup]
            command = 'echo'
            workingDir = '/directory'

            [scheduled.schedule]
            schedule = '* * * * * *'
            command = 'echo'",
        )?;
        assert_matches!(
            chronfile,
            Chronfile {
                startup_jobs,
                scheduled_jobs,
                ..
            } => {
                assert_eq!(startup_jobs.len(), 1);
                assert_matches!(startup_jobs.get("startup"), Some(StartupJob { command, working_dir, disabled, .. }) => {
                    assert_eq!(command, &"echo");
                    assert_eq!(working_dir, &Some(PathBuf::from("/directory")));
                    assert_eq!(disabled, &false);
                });
                assert_eq!(scheduled_jobs.len(), 1);
                assert_matches!(scheduled_jobs.get("schedule"), Some(ScheduledJob { schedule, working_dir, command, disabled, .. }) => {
                    assert_eq!(schedule, &"* * * * * *");
                    assert_eq!(command, &"echo");
                    assert_eq!(working_dir, &None);
                    assert_eq!(disabled, &false);
                });
            }
        );
        Ok(())
    }

    #[test]
    fn test_disabled() -> Result<()> {
        let chronfile = load_chronfile(
            "[startup.startup]
            command = 'echo'
            disabled = false

            [scheduled.schedule]
            schedule = '* * * * * *'
            command = 'echo'
            disabled = true",
        )?;
        assert_matches!(
            chronfile,
            Chronfile {
                startup_jobs,
                scheduled_jobs,
                ..
            } => {
                assert_matches!(startup_jobs.get("startup"), Some(StartupJob { disabled, .. }) => {
                    assert_eq!(disabled, &false);
                });
                assert_matches!(scheduled_jobs.get("schedule"), Some(ScheduledJob { disabled, .. }) => {
                    assert_eq!(disabled, &true);
                });
            }
        );
        Ok(())
    }

    #[test]
    fn test_extra_fields() {
        assert!(load_chronfile("foo = 'bar'").is_err());

        assert!(
            load_chronfile(
                "[startup.startup]
            command = 'echo'
            foo = 'bar'"
            )
            .is_err(),
        );

        assert!(
            load_chronfile(
                "[scheduled.schedule]
            schedule = '* * * * * *'
            command = 'echo'
            foo = 'bar'"
            )
            .is_err(),
        );

        assert!(
            load_chronfile(
                "[startup.startup]
            command = 'echo'
            retry = { foo = 'bar' }"
            )
            .is_err(),
        );

        assert!(
            load_chronfile(
                "[scheduled.schedule]
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
