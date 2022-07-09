use crate::chron_service::{
    ChronService, MakeupMissedRuns, ScheduledJobOptions, StartupJobOptions,
};
use anyhow::{Context, Result};
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer,
};
use std::{collections::HashMap, fmt, path::PathBuf};

fn default_keep_alive() -> bool {
    true
}

#[derive(Deserialize)]
pub struct StartupJob {
    command: String,
    #[serde(rename = "keepAlive", default = "default_keep_alive")]
    keep_alive: bool,
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
    fn deserialize<D>(deserializer: D) -> Result<MakeupMissedRuns, D::Error>
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
struct ScheduledJob {
    schedule: String,
    command: String,
    #[serde(rename = "makeupMissedRuns", default = "default_makeup_missed_runs")]
    makeup_missed_runs: MakeupMissedRuns,
}

#[derive(Deserialize)]
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
                        keep_alive: job.keep_alive,
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
                    },
                )
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(())
    }
}
