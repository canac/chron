#[macro_use]
extern crate diesel;
#[macro_use]
extern crate diesel_migrations;

mod chron_service;
mod database;
mod run;
mod schema;

use crate::chron_service::ChronService;
use anyhow::{Context, Result};

#[actix_web::main]
async fn main() -> Result<()> {
    let project_dirs = directories::ProjectDirs::from("com", "canac", "chron")
        .context("Failed to determine application directories")?;
    let mut chron = ChronService::new(project_dirs.data_local_dir())?;
    chron.startup("startup", "echo 'Startup'")?;
    chron.schedule("echo", "0 * * * * * *", "sleep 1; echo 'Hello world!'")?;
    chron.schedule("error", "* * * * * * *", "echo 'Hello world!'; exit 1;")?;
    chron.run()?;
    chron.start_server().await?;

    Ok(())
}
