mod models;

use self::models::{Checkpoint, Run};
use anyhow::{Context, Result};
use async_sqlite::rusqlite::{self, OptionalExtension};
use async_sqlite::{Client, ClientBuilder};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use std::path::Path;

pub struct Database {
    client: Client,
}

impl Database {
    // Create a new Database instance
    pub async fn new(chron_dir: &Path) -> Result<Self> {
        let db_path = chron_dir.join("chron.db");
        let client = ClientBuilder::new()
            .path(db_path.clone())
            .journal_mode(async_sqlite::JournalMode::Wal)
            .open()
            .await
            .with_context(|| format!("Failed to open SQLite database {db_path:?}"))?;
        client
            .conn(|conn| {
                conn.execute_batch(
                    r"CREATE TABLE IF NOT EXISTS run (
  id INTEGER PRIMARY KEY NOT NULL,
  name VARCHAR NOT NULL,
  scheduled_at DATETIME NOT NULL,
  started_at DATETIME NOT NULL DEFAULT (STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW')),
  ended_at DATETIME,
  status_code INTEGER,
  attempt INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER
);

CREATE TABLE IF NOT EXISTS checkpoint (
  id INTEGER PRIMARY KEY NOT NULL,
  job VARCHAR NOT NULL UNIQUE,
  timestamp DATETIME NOT NULL
);",
                )
            })
            .await
            .context("Failed to create SQLite tables")?;

        client
            .conn(|conn| {
                // Add a busy timeout so that when multiple processes try to write to
                // the database, they wait for each other to finish instead of erroring
                conn.execute_batch("PRAGMA busy_timeout = 1000")
            })
            .await
            .context("Failed to set busy timeout")?;
        Ok(Self { client })
    }

    // Record a new run in the database and return the id of the new run
    pub async fn insert_run(
        &self,
        name: String,
        scheduled_at: NaiveDateTime,
        attempt: usize,
        max_attempts: Option<usize>,
    ) -> Result<Run> {
        self.client
            .conn(move |conn| {
                conn.query_row(
                    "INSERT INTO run (name, scheduled_at, attempt, max_attempts) VALUES (?1, ?2, ?3, ?4) RETURNING *",
                    (name, scheduled_at, attempt, max_attempts),
                    Run::from_row,
                )
            })
            .await
            .context("Failed to save run to the database")
    }

    // Set the status code of an existing run
    pub async fn set_run_status_code(&self, run_id: u32, status_code: Option<i32>) -> Result<()> {
        self.client
            .conn(move |conn| {
                conn.execute(
                    "UPDATE run SET ended_at = STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'), status_code = ?1 WHERE id = ?2",
                    (status_code, run_id),
                )
            })
            .await
            .context("Failed to update run status in the database")?;
        Ok(())
    }

    // Read the last runs of a job
    pub async fn get_last_runs(&self, name: String, count: u64) -> Result<Vec<Run>> {
        self.client.conn(move |conn| {
            let mut statement = conn
                .prepare("SELECT id, scheduled_at, started_at, ended_at, status_code, attempt, max_attempts FROM run WHERE name = ?1 ORDER BY started_at DESC LIMIT ?2")?;
            statement.query_map(
                (name, count),
                Run::from_row,
            )?.collect::<rusqlite::Result<Vec<_>>>()
        }).await.context("Failed to load last runs from the database")
    }

    // Read the checkpoint time of a job
    pub async fn get_checkpoint(&self, job: String) -> Result<Option<DateTime<Utc>>> {
        let checkpoint = self
            .client
            .conn(|conn| {
                let mut statement =
                    conn.prepare("SELECT timestamp FROM checkpoint WHERE job = ?1")?;
                statement.query_row([job], Checkpoint::from_row).optional()
            })
            .await
            .context("Failed to save run to the database")?;
        Ok(checkpoint.map(|checkpoint| Utc.from_utc_datetime(&checkpoint.timestamp)))
    }

    // Write the checkpoint time of a job
    pub async fn set_checkpoint(&self, job: String, timestamp: DateTime<Utc>) -> Result<()> {
        self.client
            .conn(move |conn| {
                let timestamp = timestamp.naive_utc();
                let mut statement = conn.prepare("INSERT INTO checkpoint (job, timestamp) VALUES (?1, ?2) ON CONFLICT (job) DO UPDATE SET timestamp = (?2)")?;
                statement.execute((job, timestamp))
            })
            .await
            .context("Failed to save checkpoint time to the database")?;
        Ok(())
    }
}
