use crate::schema::{message, run};
use anyhow::{Context, Result};
use diesel::prelude::*;
use diesel::SqliteConnection;
use std::path::Path;

embed_migrations!();

no_arg_sql_function!(
    last_insert_rowid,
    diesel::sql_types::Integer,
    "Represents the SQL last_insert_row() function"
);

pub struct Database {
    connection: SqliteConnection,
}

impl Database {
    // Create a new Database instance
    pub fn new(chron_dir: &Path) -> Result<Self> {
        let db_path = chron_dir.join("db.sqlite");
        let connection = SqliteConnection::establish(&db_path.to_string_lossy())
            .with_context(|| format!("Error opening SQLite database {db_path:?}"))?;
        embedded_migrations::run(&connection).context("Error running SQLite migrations")?;
        Ok(Database { connection })
    }

    // Record a new run in the database and return the id of the new run
    pub fn insert_run(&self, name: &str) -> Result<i32> {
        diesel::insert_into(run::table)
            .values(run::dsl::name.eq(name))
            .execute(&self.connection)
            .context("Error saving run to database")?;
        let run_id = diesel::select(last_insert_rowid)
            .get_result::<i32>(&self.connection)
            .context("Error getting inserted run id from database")?;
        Ok(run_id)
    }

    // Set the status code of an existing run
    pub fn set_run_status_code(&self, run_id: i32, status_code: i32) -> Result<()> {
        diesel::update(run::table.find(run_id))
            .set(run::dsl::status_code.eq(status_code))
            .execute(&self.connection)
            .context("Error updating run status in the database")?;
        Ok(())
    }

    // Read the last runs of a command
    pub fn get_last_runs(&self, name: &str, count: u64) -> Result<Vec<crate::run::Run>> {
        run::table
            .filter(run::dsl::name.eq(name))
            .order(run::dsl::timestamp.desc())
            .limit(count as i64)
            .load(&self.connection)
            .context("Error loading last runs from the database")
    }

    // Read the messages in a particular mailbox
    pub fn get_messages(&self, mailbox: &str) -> Result<Vec<crate::message::Message>> {
        message::table
            .filter(message::dsl::mailbox.eq(mailbox))
            .order(message::dsl::timestamp.asc())
            .load(&self.connection)
            .context("Error loading messages from the database")
    }

    // Delete all messages in a particular mailbox
    pub fn empty_mailbox(&self, mailbox: &str) -> Result<()> {
        diesel::delete(message::table)
            .filter(message::dsl::mailbox.eq(mailbox))
            .execute(&self.connection)
            .context("Error deleting messages from the database")?;
        Ok(())
    }
}
