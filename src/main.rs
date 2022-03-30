#[macro_use]
extern crate diesel;
#[macro_use]
extern crate diesel_migrations;

mod chron_service;
mod chronfile;
mod database;
mod http_error;
mod http_server;
mod message;
mod run;
mod schema;

use crate::chron_service::ChronService;
use crate::chronfile::Chronfile;
use anyhow::{anyhow, Context, Result};

#[actix_web::main]
async fn main() -> Result<()> {
    let project_dirs = directories::ProjectDirs::from("com", "canac", "chron")
        .context("Failed to determine application directories")?;

    let port = std::env::var("PORT")
        .context("Environment variable PORT is not set")?
        .parse()
        .context("Environment variable PORT is not an integer")?;

    let args = std::env::args().collect::<Vec<_>>();
    let chronfile_path = args
        .get(1)
        .ok_or_else(|| anyhow!("No chronfile argument was provided"))?;
    let chronfile = Chronfile::load(std::path::PathBuf::from(chronfile_path))?;

    let mut chron = ChronService::new(project_dirs.data_local_dir(), port)?;
    chronfile.register_commands(&mut chron)?;
    chron.run()?;
    chron.start_server().await?;

    Ok(())
}
