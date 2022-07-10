use crate::run::Run;
use crate::schema::run;
use anyhow::{Context, Result};
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
        let db_path = chron_dir.join("db.sqlite");
        let mut connection = SqliteConnection::establish(&db_path.to_string_lossy())
            .with_context(|| format!("Error opening SQLite database {db_path:?}"))?;
        connection
            .run_pending_migrations(MIGRATIONS)
            .expect("Error running SQLite migrations");
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

    // Read the last run time of a job
    pub fn get_last_run_time(&mut self, name: &str) -> Result<Option<chrono::NaiveDateTime>> {
        let last_runs = run::table
            .filter(run::dsl::name.eq(name))
            .order(run::dsl::timestamp.desc())
            .limit(1)
            .load::<Run>(&mut self.connection)
            .context("Error loading last run time from the database")?;
        Ok(last_runs.get(0).map(|run| run.timestamp))
    }
}
