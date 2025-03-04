mod models;

use self::models::{Checkpoint, Run};
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

pub struct Database {
    connection: Connection,
}

impl Database {
    // Create a new Database instance
    pub fn new(chron_dir: &Path) -> Result<Self> {
        let db_path = chron_dir.join("chron.db");
        let connection = Connection::open(db_path.clone())
            .with_context(|| format!("Error opening SQLite database {db_path:?}"))?;
        connection
            .execute_batch(
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
            .context("Error creating SQLite tables")?;

        // Add a busy timeout so that when multiple processes try to write to
        // the database, they wait for each other to finish instead of erroring
        connection
            .execute_batch("PRAGMA busy_timeout = 1000")
            .context("Error setting busy timeout")?;
        Ok(Self { connection })
    }

    // Record a new run in the database and return the id of the new run
    pub fn insert_run(
        &self,
        name: &str,
        scheduled_at: &NaiveDateTime,
        attempt: usize,
        max_attempts: Option<usize>,
    ) -> Result<Run> {
        let mut statement = self
            .connection
            .prepare("INSERT INTO run (name, scheduled_at, attempt, max_attempts) VALUES (?1, ?2, ?3, ?4) RETURNING *")?;
        let run = statement
            .query_row((name, scheduled_at, attempt, max_attempts), Run::from_row)
            .context("Error saving run to database")?;
        Ok(run)
    }

    // Set the status code of an existing run
    pub fn set_run_status_code(&self, run_id: u32, status_code: Option<i32>) -> Result<()> {
        let mut statement = self.connection.prepare(
            "UPDATE run SET ended_at = STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'), status_code = ?1 WHERE id = ?2",
        )?;
        statement
            .execute((status_code, run_id))
            .context("Error updating run status in the database")?;
        Ok(())
    }

    // Read the last runs of a job
    #[allow(clippy::cast_possible_wrap)]
    pub fn get_last_runs(&self, name: &str, count: u64) -> Result<Vec<Run>> {
        let mut statement = self
            .connection
            .prepare("SELECT id, scheduled_at, started_at, ended_at, status_code, attempt, max_attempts FROM run WHERE name = ?1 ORDER BY started_at DESC LIMIT ?2")?;
        let runs = statement
            .query_map((name, count), Run::from_row)
            .context("Error loading last runs from the database")?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(runs)
    }

    // Read the checkpoint time of a job
    pub fn get_checkpoint(&self, job: &str) -> Result<Option<DateTime<Utc>>> {
        let mut statement = self
            .connection
            .prepare("SELECT timestamp FROM checkpoint WHERE job = ?1")?;
        let checkpoint = statement
            .query_row([job], Checkpoint::from_row)
            .optional()
            .context("Error saving run to database")?;
        Ok(checkpoint.map(|checkpoint| Utc.from_utc_datetime(&checkpoint.timestamp)))
    }

    // Write the checkpoint time of a job
    pub fn set_checkpoint(&self, job: &str, timestamp: DateTime<Utc>) -> Result<()> {
        let timestamp = timestamp.naive_utc();
        let mut statement = self
            .connection
            .prepare("INSERT INTO checkpoint (job, timestamp) VALUES (?1, ?2) ON CONFLICT (job) DO UPDATE SET timestamp = (?2)")?;
        statement
            .execute((job, timestamp))
            .context("Error saving checkpoint time to the database")?;
        Ok(())
    }
}
