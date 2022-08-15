#[macro_use]
extern crate diesel;
#[macro_use]
extern crate diesel_migrations;

mod chron_service;
mod chronfile;
mod cli;
mod database;
mod http;
mod sleep;

use crate::chron_service::ChronService;
use crate::chronfile::Chronfile;
use crate::cli::Cli;
use anyhow::{Context, Result};
use clap::Parser;
use log::{debug, error, LevelFilter};
use notify::{DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};
use std::thread;
use std::time::Duration;

#[actix_web::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    simple_logger::SimpleLogger::new()
        .with_module_level("actix_server", LevelFilter::Off)
        .with_module_level("mio", LevelFilter::Off)
        .with_level(if cli.quiet {
            LevelFilter::Warn
        } else {
            LevelFilter::Debug
        })
        .init()?;

    let chronfile_path = cli.chronfile;
    let chronfile = Chronfile::load(chronfile_path.clone())?;

    let project_dirs = directories::ProjectDirs::from("com", "canac", "chron")
        .context("Failed to determine application directories")?;
    let chron_lock = ChronService::new(project_dirs.data_local_dir())?;
    chronfile.run(&mut chron_lock.write().unwrap())?;

    let chron = chron_lock.clone();
    thread::spawn(move || -> Result<()> {
        // Watch for changes to the chronfile
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher: RecommendedWatcher = Watcher::new(tx, Duration::from_secs(1))
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

    match cli.port {
        // Start the server, which will wait indefinitely
        Some(port) => http::start_server(chron_lock, port).await?,
        // When there is no server, wait forever
        None => loop {
            thread::sleep(Duration::MAX);
        },
    }

    Ok(())
}
