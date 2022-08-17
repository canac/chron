mod models;
mod schema;

use self::models::{Checkpoint, Run};
use self::schema::{checkpoint, run};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness};
use std::path::Path;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

pub struct Database {
    connection: SqliteConnection,
}

impl Database {
    // Create a new Database instance
    pub fn new(chron_dir: &Path) -> Result<Self> {
        let db_path = chron_dir.join("chron.db");
        let mut connection = SqliteConnection::establish(&db_path.to_string_lossy())
            .with_context(|| format!("Error opening SQLite database {db_path:?}"))?;
        connection
            .run_pending_migrations(MIGRATIONS)
            .expect("Error running SQLite migrations");
        // Add a busy timeout so that when multiple processes try to write to
        // the database, they wait for each other to finish instead of erroring
        connection
            .batch_execute("PRAGMA busy_timeout = 1000")
            .context("Error setting busy timeout")?;
        Ok(Database { connection })
    }

    // Record a new run in the database and return the id of the new run
    pub fn insert_run(&mut self, name: &str) -> Result<Run> {
        let run = diesel::insert_into(run::table)
            .values(run::dsl::name.eq(name))
            .get_result(&mut self.connection)
            .context("Error saving run to database")?;
        Ok(run)
    }

    // Set the status code of an existing run
    pub fn set_run_status_code(&mut self, run_id: i32, status_code: i32) -> Result<()> {
        diesel::update(run::table.find(run_id))
            .set(run::dsl::status_code.eq(status_code))
            .execute(&mut self.connection)
            .context("Error updating run status in the database")?;
        Ok(())
    }

    // Read the last runs of a job
    pub fn get_last_runs(&mut self, name: &str, count: u64) -> Result<Vec<Run>> {
        run::table
            .filter(run::dsl::name.eq(name))
            .order(run::dsl::timestamp.desc())
            .limit(count as i64)
            .load(&mut self.connection)
            .context("Error loading last runs from the database")
    }

    // Read the checkpoint time of a job
    pub fn get_checkpoint(&mut self, job: &str) -> Result<Option<DateTime<Utc>>> {
        let checkpoints = checkpoint::table
            .filter(checkpoint::dsl::job.eq(job))
            .limit(1)
            .load::<Checkpoint>(&mut self.connection)
            .context("Error loading checkpoint time from the database")?;
        Ok(checkpoints
            .get(0)
            .map(|checkpoint| Utc.from_utc_datetime(&checkpoint.timestamp)))
    }

    // Write the checkpoint time of a job
    pub fn set_checkpoint(&mut self, job: &str, timestamp: DateTime<Utc>) -> Result<()> {
        let timestamp = timestamp.naive_utc();
        diesel::insert_into(checkpoint::table)
            .values((
                checkpoint::dsl::job.eq(job.to_string()),
                checkpoint::dsl::timestamp.eq(timestamp),
            ))
            .on_conflict(checkpoint::dsl::job)
            .do_update()
            .set(checkpoint::dsl::timestamp.eq(timestamp))
            .execute(&mut self.connection)
            .context("Error saving checkpoint time to the database")?;
        Ok(())
    }
}
