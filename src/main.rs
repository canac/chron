#![warn(
    clippy::clone_on_ref_ptr,
    clippy::str_to_string,
    clippy::pedantic,
    clippy::nursery
)]
#![allow(clippy::missing_const_for_fn)]

mod chron_service;
mod chronfile;
mod cli;
mod database;
mod http;
mod sync_ext;

use crate::chron_service::ChronService;
use crate::chronfile::Chronfile;
use crate::cli::Cli;
use anyhow::{Context, Result};
use clap::Parser;
use log::{LevelFilter, debug, error, info};
use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use std::io::{IsTerminal, stdin};
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::oneshot::channel;

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
    ChronService::start(&chron_lock, chronfile)?;

    let watcher_chron = Arc::clone(&chron_lock);
    let watch_path = chronfile_path.clone();
    let mut debouncer = new_debouncer(Duration::from_secs(1), move |res: DebounceEventResult| {
        if res.is_err() {
            return;
        }

        match Chronfile::load(&watch_path) {
            Ok(chronfile) => {
                debug!("Reloaded chronfile {}", watch_path.to_string_lossy());
                if let Err(err) = ChronService::start(&watcher_chron, chronfile) {
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

    let (tx, rx) = channel();
    let ctrlc_chron = Arc::clone(&chron_lock);
    let mut tx = Some(tx);
    let second_signal = AtomicBool::new(false);
    ctrlc::set_handler(move || {
        if second_signal.swap(true, Ordering::Relaxed) {
            info!("Shutting down forcefully");
            exit(1);
        }

        info!("Shutting down gracefully...");
        if stdin().is_terminal() {
            info!("To shut down immediately, press Ctrl-C again");
        }
        ChronService::stop(&ctrlc_chron);
        if let Some(tx) = tx.take() {
            tx.send(()).unwrap();
        }
    })?;

    match cli.port {
        Some(port) => http::create_server(chron_lock, port, rx).await?,
        // Wait for termination signal
        None => rx.await?,
    }

    Ok(())
}
