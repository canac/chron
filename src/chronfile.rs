use crate::chron_service::{
    ChronService, MakeupMissedRuns, RetryConfig, ScheduledJobOptions, StartupJobOptions,
};
use anyhow::{Context, Result};
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer,
};
use std::{collections::HashMap, fmt, path::PathBuf, time::Duration};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRetryConfigInternal {
    failures: Option<bool>,
    successes: Option<bool>,
    limit: Option<u64>,
    #[serde(default, with = "humantime_serde")]
    delay: Option<Duration>,
}

// Allow RawRetryConfig to be deserialized from a boolean or a full retry config
#[derive(Deserialize)]
#[serde(deny_unknown_fields, untagged)]
enum RetryConfigVariant {
    Simple(bool),
    Complex(RawRetryConfigInternal),
}

impl Default for RetryConfigVariant {
    fn default() -> Self {
        RetryConfigVariant::Complex(Default::default())
    }
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
            RetryConfigVariant::Complex(config) => RawRetryConfig {
                failures: config.failures,
                successes: config.successes,
                limit: config.limit,
                delay: config.delay,
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

struct AllOrCountVisitor;

impl<'de> Visitor<'de> for AllOrCountVisitor {
    type Value = MakeupMissedRuns;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("string literal \"all\" or a positive integer")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value == "all" {
            Ok(MakeupMissedRuns::All)
        } else {
            Err(E::custom(format!(
                "string literal is not \"all\": {}",
                value
            )))
        }
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value >= 0 {
            Ok(MakeupMissedRuns::Count(value as u64))
        } else {
            Err(E::custom(format!("i64 is not positive: {}", value)))
        }
    }
}

impl<'de> Deserialize<'de> for MakeupMissedRuns {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(AllOrCountVisitor)
    }
}

fn default_makeup_missed_runs() -> MakeupMissedRuns {
    MakeupMissedRuns::Count(0)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScheduledJob {
    schedule: String,
    command: String,
    #[serde(default = "default_makeup_missed_runs")]
    makeup_missed_runs: MakeupMissedRuns,
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
                        makeup_missed_runs: job.makeup_missed_runs,
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
