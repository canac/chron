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
mod schema;
mod terminate_controller;

use crate::chron_service::ChronService;
use crate::chronfile::Chronfile;
use anyhow::{anyhow, Context, Result};
use notify::{DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};

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
    let chronfile_path = std::path::PathBuf::from(chronfile_path);
    let chronfile = Chronfile::load(chronfile_path.clone())?;

    let chron_lock = ChronService::new(project_dirs.data_local_dir())?;
    chronfile.run(&mut chron_lock.write().unwrap())?;

    let chron = chron_lock.clone();
    std::thread::spawn(move || -> Result<()> {
        // Watch for changes to the chronfile
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher: RecommendedWatcher = Watcher::new(tx, std::time::Duration::from_secs(5))
            .context("Failed to create chronfile watcher")?;
        watcher
            .watch(chronfile_path.clone(), RecursiveMode::NonRecursive)
            .context("Failed to start chronfile watcher")?;
        loop {
            let event = rx.recv().context("Failed to read watcher receiver")?;
            if let DebouncedEvent::Write(_) = event {
                // Reload the chronfile
                eprintln!("Reloading chronfile {chronfile_path:?} ...");
                let chronfile = Chronfile::load(chronfile_path.clone())?;
                chronfile.run(&mut *chron.write().unwrap())?;
            }
        }
    });

    http_server::start_server(chron_lock, port).await?;

    Ok(())
}
