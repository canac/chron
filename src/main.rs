#![warn(
    clippy::clone_on_ref_ptr,
    clippy::str_to_string,
    clippy::pedantic,
    clippy::nursery
)]
#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![allow(clippy::missing_const_for_fn)]

mod chron_service;
mod chronfile;
mod cli;
mod database;
mod http;
mod result_ext;

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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tokio::sync::oneshot::channel;

#[actix_web::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let port = cli.port;

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
    let chronfile = Chronfile::load(&chronfile_path).await?;

    let project_dirs = directories::ProjectDirs::from("com", "canac", "chron")
        .context("Failed to determine application directories")?;
    let chron = ChronService::new(project_dirs.data_local_dir()).await?;
    let chron_lock = Arc::new(RwLock::new(chron));
    let server = http::create_server(Arc::clone(&chron_lock), port).await?;
    chron_lock.write().await.start(chronfile).await?;

    let watcher_chron = Arc::clone(&chron_lock);
    let watch_path = chronfile_path.clone();
    let handle = Handle::current();
    let mut debouncer = new_debouncer(Duration::from_secs(1), move |res: DebounceEventResult| {
        if res.is_err() {
            return;
        }

        handle.block_on(async {
            match Chronfile::load(&watch_path).await {
                Ok(chronfile) => {
                    debug!("Reloading chronfile {}", watch_path.to_string_lossy());
                    if let Err(err) = watcher_chron.write().await.start(chronfile).await {
                        error!("Failed to start chron\n{err:?}");
                    }
                }
                Err(err) => error!(
                    "Failed to parse chronfile {}\n{err:?}",
                    watch_path.to_string_lossy()
                ),
            }
        });
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
    let handle = Handle::current();
    ctrlc::set_handler(move || {
        if second_signal.swap(true, Ordering::Relaxed) {
            info!("Shutting down forcefully");
            exit(1);
        }

        info!("Shutting down gracefully...");
        if stdin().is_terminal() {
            info!("To shut down immediately, press Ctrl-C again");
        }
        handle.block_on(async { ctrlc_chron.write().await.stop().await });
        if let Some(tx) = tx.take() {
            tx.send(()).expect("Failed to send terminate message");
        }
    })?;

    // Start the HTTP server
    let handle = server.handle();
    tokio::select! {
        _ = server => (),
        _ = rx => {
            info!("Stopping HTTP server");
            handle.stop(true).await;
        },
    }

    Ok(())
}
