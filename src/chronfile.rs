use crate::chron_service::ChronService;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

#[derive(Deserialize)]
pub struct StartupJob {
    command: String,
}

#[derive(Deserialize)]
struct ScheduledJob {
    schedule: String,
    command: String,
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
    pub fn run(&self, chron: &mut ChronService) -> Result<()> {
        chron.reset()?;

        self.startup_jobs
            .iter()
            .map(|(name, job)| chron.startup(name, &job.command))
            .collect::<Result<Vec<_>>>()?;

        self.scheduled_jobs
            .iter()
            .map(|(name, job)| chron.schedule(name, &job.schedule, &job.command))
            .collect::<Result<Vec<_>>>()?;

        chron.start()
    }
}
