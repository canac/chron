mod scheduled_job;
mod startup_job;

use self::scheduled_job::ScheduledJob;
use self::startup_job::StartupJob;
use crate::chron_service::ChronService;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chronfile {
    #[serde(rename = "startup", default)]
    startup_jobs: HashMap<String, StartupJob>,
    #[serde(rename = "schedule", default)]
    scheduled_jobs: HashMap<String, ScheduledJob>,
}

impl Chronfile {
    // Load a chronfile
    pub fn load(path: PathBuf) -> Result<Self> {
        let toml_str = std::fs::read_to_string(&path)
            .with_context(|| format!("Error reading chronfile {path:?}"))?;
        toml::from_str(&toml_str)
            .with_context(|| format!("Error deserializing TOML chronfile {path:?}"))
    }

    // Register the chronfile's jobs with a ChronService instance and start it
    pub fn run(self, chron: &mut ChronService) -> Result<()> {
        chron.reset()?;

        self.startup_jobs
            .into_iter()
            .filter(|(_, job)| !job.disabled)
            .try_for_each(|(name, job)| chron.startup(&name, &job.command, job.get_options()))?;

        self.scheduled_jobs
            .into_iter()
            .filter(|(_, job)| !job.disabled)
            .try_for_each(|(name, job)| {
                chron.schedule(&name, &job.schedule, &job.command, job.get_options())
            })?;

        Ok(())
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
        assert!(load_chronfile("").is_ok())
    }

    #[test]
    fn test_simple() -> Result<()> {
        let chronfile = load_chronfile(
            "[startup.startup]
            command = 'echo'

            [schedule.schedule]
            schedule = '* * * * * *'
            command = 'echo'",
        )?;
        assert_matches!(
            chronfile,
            Chronfile {
                startup_jobs,
                scheduled_jobs,
            } => {
                assert_eq!(startup_jobs.len(), 1);
                assert_matches!(startup_jobs.get("startup"), Some(StartupJob { command, disabled, .. }) => {
                    assert_eq!(command, &"echo");
                    assert_eq!(disabled, &false);
                });
                assert_eq!(scheduled_jobs.len(), 1);
                assert_matches!(scheduled_jobs.get("schedule"), Some(ScheduledJob { schedule, command, disabled, .. }) => {
                    assert_eq!(schedule, &"* * * * * *");
                    assert_eq!(command, &"echo");
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

            [schedule.schedule]
            schedule = '* * * * * *'
            command = 'echo'
            disabled = true",
        )?;
        assert_matches!(
            chronfile,
            Chronfile {
                startup_jobs,
                scheduled_jobs,
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

        assert!(load_chronfile(
            "[startup.startup]
            command = 'echo'
            foo = 'bar'"
        )
        .is_err());

        assert!(load_chronfile(
            "[schedule.schedule]
            schedule = '* * * * * *'
            command = 'echo'
            foo = 'bar'"
        )
        .is_err());

        assert!(load_chronfile(
            "[startup.startup]
            command = 'echo'
            keepAlive = { foo = 'bar' }"
        )
        .is_err());

        assert!(load_chronfile(
            "[schedule.schedule]
            schedule = '* * * * * *'
            command = 'echo'
            retry = { foo = 'bar' }"
        )
        .is_err());
    }
}
