#![warn(
    clippy::clone_on_ref_ptr,
    clippy::str_to_string,
    clippy::pedantic,
    clippy::nursery
)]
#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![allow(clippy::cognitive_complexity, clippy::missing_const_for_fn)]

mod chron_service;
mod chronfile;
mod cli;
mod commands;
mod database;
mod format;
mod http;
mod http_helpers;
mod result_ext;

use crate::{cli::Cli, database::ClientDatabase};
use anyhow::{Context, Result};
use clap::Parser;
use cli::Command;
use std::{path::PathBuf, sync::Arc};
use tokio::fs::create_dir_all;

#[actix_web::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Make sure that the chron directory exists
    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => {
            if std::env::var("CARGO").is_ok() {
                // Default to storing files in ./data during development
                PathBuf::from("data")
            } else {
                directories::ProjectDirs::from("com", "canac", "chron")
                    .context("Failed to determine application directories")?
                    .data_local_dir()
                    .to_owned()
            }
        }
    };
    create_dir_all(&data_dir).await?;

    match cli.command {
        Command::Run(args) => {
            commands::run(&data_dir, args).await?;
        }
        Command::Jobs => {
            let db = Arc::new(ClientDatabase::open(&data_dir).await?);
            commands::jobs(db).await?;
        }
        Command::Status(args) => {
            let db = Arc::new(ClientDatabase::open(&data_dir).await?);
            commands::status(db, args).await?;
        }
        Command::Runs(args) => {
            let db = Arc::new(ClientDatabase::open(&data_dir).await?);
            commands::runs(db, args).await?;
        }
        Command::Logs(args) => {
            let db = Arc::new(ClientDatabase::open(&data_dir).await?);
            commands::logs(db, args).await?;
        }
        Command::Kill(args) => {
            let db = Arc::new(ClientDatabase::open(&data_dir).await?);
            commands::kill(db, args).await?;
        }
    }

    Ok(())
}
