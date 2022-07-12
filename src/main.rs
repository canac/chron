#[macro_use]
extern crate diesel;
#[macro_use]
extern crate diesel_migrations;

mod chron_service;
mod chronfile;
mod database;
mod http_error;
mod http_server;
mod run;
mod scheduled_job;
mod schema;
mod terminate_controller;

use crate::chron_service::ChronService;
use crate::chronfile::Chronfile;
use anyhow::{anyhow, Context, Result};
use log::{debug, error, LevelFilter};
use notify::{DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};

#[actix_web::main]
async fn main() -> Result<()> {
    simple_logger::SimpleLogger::new()
        .with_module_level("actix_server", LevelFilter::Off)
        .with_module_level("mio", LevelFilter::Off)
        .with_level(LevelFilter::Debug)
        .init()?;

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
    let chronfile_path = std::path::PathBuf::from(chronfile_path);
    let chronfile = Chronfile::load(chronfile_path.clone())?;

    let chron_lock = ChronService::new(project_dirs.data_local_dir())?;
    chronfile.run(&mut chron_lock.write().unwrap())?;

    let chron = chron_lock.clone();
    std::thread::spawn(move || -> Result<()> {
        // Watch for changes to the chronfile
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher: RecommendedWatcher = Watcher::new(tx, std::time::Duration::from_secs(1))
            .context("Failed to create chronfile watcher")?;
        watcher
            .watch(chronfile_path.clone(), RecursiveMode::NonRecursive)
            .context("Failed to start chronfile watcher")?;
        loop {
            let event = rx.recv().context("Failed to read watcher receiver")?;
            if let DebouncedEvent::Write(_) = event {
                // Reload the chronfile
                match Chronfile::load(chronfile_path.clone()) {
                    Ok(chronfile) => {
                        debug!("Reloaded chronfile {}", chronfile_path.to_string_lossy());
                        chronfile.run(&mut *chron.write().unwrap())?;
                    }
                    Err(err) => error!(
                        "Error reloading chronfile {}\n{err:?}",
                        chronfile_path.to_string_lossy()
                    ),
                };
            }
        }
    });

    http_server::start_server(chron_lock, port).await?;

    Ok(())
}
