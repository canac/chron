#[macro_use]
extern crate diesel;
#[macro_use]
extern crate diesel_migrations;

mod chron_service;
mod job_scheduler;
mod run;
mod schema;

use crate::chron_service::ChronService;
use anyhow::{Context, Result};

fn main() -> Result<()> {
    let project_dirs = directories::ProjectDirs::from("com", "canac", "chron")
        .context("Failed to determine application directories")?;
    let mut chron = ChronService::new(project_dirs.data_local_dir())?;
    chron.startup("startup", "echo 'Startup'")?;
    chron.schedule("echo", "* * * * * * *", "echo 'Hello world!'")?;
    chron.run()?;

    Ok(())
}
