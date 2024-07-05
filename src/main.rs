#![warn(clippy::clone_on_ref_ptr, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod chron_service;
mod chronfile;
mod cli;
mod database;
mod http;

use crate::chron_service::ChronService;
use crate::chronfile::Chronfile;
use crate::cli::Cli;
use anyhow::{Context, Result};
use clap::Parser;
use log::{debug, error, LevelFilter};
use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;
use std::sync::Arc;
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
    let chronfile = Chronfile::load(&chronfile_path)?;

    let project_dirs = directories::ProjectDirs::from("com", "canac", "chron")
        .context("Failed to determine application directories")?;
    let chron_lock = ChronService::new(project_dirs.data_local_dir())?;
    chron_lock.write().unwrap().start(chronfile)?;

    let chron = Arc::clone(&chron_lock);
    thread::spawn(move || -> Result<()> {
        // Watch for changes to the chronfile
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = new_debouncer(Duration::from_secs(1), None, tx)
            .context("Failed to create watcher debouncer")?;
        debouncer
            .watcher()
            .watch(&chronfile_path, RecursiveMode::NonRecursive)
            .context("Failed to start chronfile watcher")?;
        loop {
            let events = rx.recv().context("Failed to read watcher receiver")?;
            if events.is_ok() {
                // Reload the chronfile
                match Chronfile::load(&chronfile_path) {
                    Ok(chronfile) => {
                        debug!("Reloaded chronfile {}", chronfile_path.to_string_lossy());
                        chron.write().unwrap().start(chronfile)?;
                    }
                    Err(err) => error!(
                        "Error reloading chronfile {}\n{err:?}",
                        chronfile_path.to_string_lossy()
                    ),
                };
            };
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
