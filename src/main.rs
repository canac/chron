#![warn(
    clippy::clone_on_ref_ptr,
    clippy::str_to_string,
    clippy::pedantic,
    clippy::nursery
)]
#![allow(clippy::future_not_send, clippy::missing_const_for_fn)]

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
use log::{LevelFilter, debug, error};
use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use std::sync::{Arc, RwLock};
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
    let chron_lock = Arc::new(RwLock::new(ChronService::new(
        project_dirs.data_local_dir(),
    )?));
    chron_lock.write().unwrap().start(chronfile)?;

    let chron = Arc::clone(&chron_lock);
    let watch_path = chronfile_path.clone();
    let mut debouncer = new_debouncer(Duration::from_secs(1), move |res: DebounceEventResult| {
        if res.is_err() {
            return;
        }

        match Chronfile::load(&watch_path) {
            Ok(chronfile) => {
                debug!("Reloaded chronfile {}", watch_path.to_string_lossy());
                #[allow(clippy::significant_drop_in_scrutinee)]
                if let Err(err) = chron.write().unwrap().start(chronfile) {
                    error!("Error starting chron\n{err:?}");
                }
            }
            Err(err) => error!(
                "Error reloading chronfile {}\n{err:?}",
                watch_path.to_string_lossy()
            ),
        }
    })
    .context("Failed to create watcher debouncer")?;
    debouncer
        .watcher()
        .watch(&chronfile_path, RecursiveMode::NonRecursive)
        .context("Failed to start chronfile watcher")?;

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
