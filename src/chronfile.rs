use crate::chron_service::{ChronService, RetryConfig, ScheduledJobOptions, StartupJobOptions};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf, time::Duration};

#[derive(Deserialize)]
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

#[derive(Default, Deserialize)]
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StartupJob {
    command: String,
    #[serde(default)]
    keep_alive: RawRetryConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScheduledJob {
    schedule: String,
    command: String,
    #[serde(default)]
    make_up_missed_runs: MakeUpRunsVariant,
    #[serde(default)]
    retry: RawRetryConfig,
}

#[derive(Deserialize)]
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
            .map(|(name, job)| {
                chron.startup(
                    &name,
                    &job.command,
                    StartupJobOptions {
                        keep_alive: RetryConfig {
                            failures: job.keep_alive.failures.unwrap_or(true),
                            successes: job.keep_alive.successes.unwrap_or(true),
                            limit: job.keep_alive.limit,
                            delay: job
                                .keep_alive
                                .delay
                                .unwrap_or_else(|| Duration::from_secs(0)),
                        },
                    },
                )
            })
            .collect::<Result<Vec<_>>>()?;

        self.scheduled_jobs
            .into_iter()
            .map(|(name, job)| {
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
                            delay: job.retry.delay.unwrap_or_else(|| Duration::from_secs(60)),
                        },
                    },
                )
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(())
    }
}
