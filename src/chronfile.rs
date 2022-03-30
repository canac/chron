use crate::chron_service::ChronService;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

#[derive(Deserialize)]
pub struct StartupCommand {
    command: String,
}

#[derive(Deserialize)]
struct ScheduledCommand {
    schedule: String,
    command: String,
}

#[derive(Deserialize)]
pub struct Chronfile {
    #[serde(rename = "startup", default)]
    startup_commands: HashMap<String, StartupCommand>,
    #[serde(rename = "schedule", default)]
    scheduled_commands: HashMap<String, ScheduledCommand>,
}

impl Chronfile {
    // Load a chronfile
    pub fn load(path: PathBuf) -> Result<Self> {
        let toml_str = std::fs::read_to_string(&path)
            .with_context(|| format!("Error reading chronfile {path:?}"))?;
        toml::from_str(&toml_str)
            .with_context(|| format!("Error deserializing TOML chronfile {path:?}"))
    }

    // Register the chronfile's commands with a ChronService instance
    pub fn register_commands(&self, chron: &mut ChronService) -> Result<()> {
        self.startup_commands
            .iter()
            .map(|(name, command)| chron.startup(name, &command.command))
            .collect::<Result<Vec<_>>>()?;

        self.scheduled_commands
            .iter()
            .map(|(name, command)| chron.schedule(name, &command.schedule, &command.command))
            .collect::<Result<Vec<_>>>()?;

        Ok(())
    }
}
