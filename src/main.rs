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
mod commands;
mod database;
mod format;
mod http;
mod result_ext;

use crate::cli::Cli;
use anyhow::Result;
use clap::Parser;
use cli::Command;
use commands::get_data_dir;
use database::Database;
use std::sync::Arc;
use tokio::fs::create_dir_all;

#[actix_web::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Make sure that the chron directory exists
    let data_dir = get_data_dir()?;
    create_dir_all(&data_dir).await?;

    let db = Arc::new(Database::new(&data_dir).await?);
    match cli.command {
        Command::Run(args) => {
            commands::run(db, args).await?;
        }
        Command::Status(args) => {
            commands::status(db, args).await?;
        }
        Command::Runs(args) => {
            commands::runs(db, args).await?;
        }
        Command::Logs(args) => {
            commands::logs(db, args).await?;
        }
        Command::Kill(args) => {
            commands::kill(db, args).await?;
        }
    }

    Ok(())
}
