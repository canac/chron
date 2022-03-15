mod chron_service;
mod job_scheduler;

use crate::chron_service::ChronService;
use anyhow::{Context, Result};

fn main() -> Result<()> {
    let project_dirs = directories::ProjectDirs::from("com", "canac", "chron")
        .context("Failed to determine application directories")?;
    let mut chron = ChronService::new(project_dirs.data_local_dir());
    chron.add("echo", "* * * * * * *", "echo 'Hello world!'")?;
    chron.run();

    Ok(())
}
