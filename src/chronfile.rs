use crate::chron_service::{ChronService, RetryConfig, ScheduledJobOptions, StartupJobOptions};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf, time::Duration};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
enum MakeUpRunsVariant {
    Simple(bool),
    Complex(u64),
}

impl Default for MakeUpRunsVariant {
    fn default() -> Self {
        MakeUpRunsVariant::Simple(false)
    }
}

impl From<MakeUpRunsVariant> for u64 {
    fn from(val: MakeUpRunsVariant) -> Self {
        match val {
            MakeUpRunsVariant::Simple(false) => 0,
            MakeUpRunsVariant::Simple(true) => u64::MAX,
            MakeUpRunsVariant::Complex(limit) => limit,
        }
    }
}

// Allow RawRetryConfig to be deserialized from a boolean or a full retry config
#[derive(Deserialize)]
#[serde(deny_unknown_fields, untagged)]
enum RetryConfigVariant {
    Simple(bool),
    Complex {
        failures: Option<bool>,
        successes: Option<bool>,
        limit: Option<u64>,
        #[serde(default, with = "humantime_serde")]
        delay: Option<Duration>,
    },
}

#[derive(Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(from = "RetryConfigVariant")]
struct RawRetryConfig {
    failures: Option<bool>,
    successes: Option<bool>,
    limit: Option<u64>,
    delay: Option<Duration>,
}

impl From<RetryConfigVariant> for RawRetryConfig {
    fn from(value: RetryConfigVariant) -> Self {
        match value {
            RetryConfigVariant::Simple(value) => RawRetryConfig {
                failures: Some(value),
                successes: Some(value),
                ..Default::default()
            },
            RetryConfigVariant::Complex {
                failures,
                successes,
                limit,
                delay,
            } => RawRetryConfig {
                failures,
                successes,
                limit,
                delay,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StartupJob {
    command: String,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    keep_alive: RawRetryConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScheduledJob {
    schedule: String,
    command: String,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    make_up_missed_runs: MakeUpRunsVariant,
    #[serde(default)]
    retry: RawRetryConfig,
}

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
            .try_for_each(|(name, job)| {
                chron.startup(
                    &name,
                    &job.command,
                    StartupJobOptions {
                        keep_alive: RetryConfig {
                            failures: job.keep_alive.failures.unwrap_or(true),
                            successes: job.keep_alive.successes.unwrap_or(true),
                            limit: job.keep_alive.limit,
                            delay: job.keep_alive.delay,
                        },
                    },
                )
            })?;

        self.scheduled_jobs
            .into_iter()
            .filter(|(_, job)| !job.disabled)
            .try_for_each(|(name, job)| {
                chron.schedule(
                    &name,
                    &job.schedule,
                    &job.command,
                    ScheduledJobOptions {
                        make_up_missed_runs: job.make_up_missed_runs.into(),
                        retry: RetryConfig {
                            failures: job.retry.failures.unwrap_or(true),
                            successes: job.retry.successes.unwrap_or(false),
                            limit: job.retry.limit,
                            delay: job.retry.delay,
                        },
                    },
                )
            })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
    fn test_keep_alive() -> Result<()> {
        assert_eq!(
            toml::from_str::<StartupJob>("command = 'echo'\nkeepAlive = false")?.keep_alive,
            RawRetryConfig {
                failures: Some(false),
                successes: Some(false),
                ..Default::default()
            }
        );

        assert_eq!(
            toml::from_str::<StartupJob>("command = 'echo'\nkeepAlive = true")?.keep_alive,
            RawRetryConfig {
                failures: Some(true),
                successes: Some(true),
                ..Default::default()
            }
        );

        assert_eq!(
            toml::from_str::<StartupJob>(
                "command = 'echo'\nkeepAlive = { successes = true, limit = 3 }"
            )?
            .keep_alive,
            RawRetryConfig {
                failures: None,
                successes: Some(true),
                limit: Some(3),
                delay: None,
            }
        );

        Ok(())
    }

    #[test]
    fn test_makeup_missed_runs() -> Result<()> {
        let make_up_missed_runs: u64 = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRuns = false",
        )?
        .make_up_missed_runs
        .into();
        assert_eq!(make_up_missed_runs, 0);

        let make_up_missed_runs: u64 = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRuns = true",
        )?
        .make_up_missed_runs
        .into();
        assert_eq!(make_up_missed_runs, u64::MAX);

        let make_up_missed_runs: u64 = toml::from_str::<ScheduledJob>(
            "command = 'echo'\nschedule = '* * * * * *'\nmakeUpMissedRuns = 3",
        )?
        .make_up_missed_runs
        .into();
        assert_eq!(make_up_missed_runs, 3);

        Ok(())
    }

    #[test]
    fn test_retry() -> Result<()> {
        assert_eq!(
            toml::from_str::<ScheduledJob>(
                "command = 'echo'\nschedule = '* * * * * *'\nretry = false"
            )?
            .retry,
            RawRetryConfig {
                failures: Some(false),
                successes: Some(false),
                ..Default::default()
            }
        );

        assert_eq!(
            toml::from_str::<ScheduledJob>(
                "command = 'echo'\nschedule = '* * * * * *'\nretry = true"
            )?
            .retry,
            RawRetryConfig {
                failures: Some(true),
                successes: Some(true),
                ..Default::default()
            }
        );

        assert_eq!(
            toml::from_str::<ScheduledJob>(
                "command = 'echo'\nschedule = '* * * * * *'\nretry = { failures = true, delay = '10m' }"
            )?
            .retry,
            RawRetryConfig {
                failures: Some(true),
                successes: None,
                limit: None,
                delay: Some(Duration::from_secs(600)),
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
