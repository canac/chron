use crate::job_scheduler::{Job, JobScheduler};
use crate::schema::run;
use anyhow::{bail, Context, Result};
use cron::Schedule;
use diesel::prelude::*;
use diesel::SqliteConnection;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;

embed_migrations!();

no_arg_sql_function!(
    last_insert_rowid,
    diesel::sql_types::Integer,
    "Represents the SQL last_insert_row() function"
);

pub struct ChronService<'job> {
    log_dir: PathBuf,
    scheduled_jobs: HashMap<String, Job<'job>>,
    // The key is the name and the value is the command to run
    startup_commands: HashMap<String, String>,
    scheduler: JobScheduler<'job>,
    db_connection: Arc<Mutex<SqliteConnection>>,
}

impl<'job> ChronService<'job> {
    // Create a new ChronService instance
    pub fn new(chron_dir: &Path) -> Result<Self> {
        let db_path = chron_dir.join("db.sqlite");
        let connection = SqliteConnection::establish(&db_path.to_string_lossy())
            .with_context(|| format!("Error opening SQLite database {db_path:?}"))?;
        embedded_migrations::run(&connection).context("Error running SQLite migrations")?;

        Ok(ChronService {
            log_dir: chron_dir.join("logs"),
            scheduled_jobs: HashMap::new(),
            startup_commands: HashMap::new(),
            scheduler: JobScheduler::new(),
            db_connection: Arc::new(Mutex::new(connection)),
        })
    }

    // Add a new command to be run on startup
    pub fn startup(&mut self, name: &str, command: &str) -> Result<()> {
        if !Self::validate_name(name) {
            bail!("Invalid command name {name}")
        }

        if self.startup_commands.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }
        self.startup_commands
            .insert(name.to_string(), command.to_string());

        Ok(())
    }

    // Add a new command to be run on the given schedule
    pub fn schedule<'cmd>(
        &mut self,
        name: &'job str,
        schedule_expression: &str,
        command: &'cmd str,
    ) -> Result<()>
    where
        'cmd: 'job,
    {
        if !Self::validate_name(name) {
            bail!("Invalid command name {name}")
        }

        if self.scheduled_jobs.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let schedule = Schedule::from_str(schedule_expression).with_context(|| {
            format!("Failed to parse schedule expression {schedule_expression}")
        })?;
        let log_dir = self.log_dir.clone();
        let db_connection = self.db_connection.clone();
        let job = Job::new(schedule, move || {
            Self::exec_command(&db_connection, &log_dir, name, command).unwrap()
        });
        self.scheduler.add(job);
        // self.scheduled_jobs.insert(name.to_string(), job);

        Ok(())
    }

    // Start the chron service, running all scripts on their specified interval
    // Note that this function starts an infinite loop and will never return
    pub fn run(mut self) -> Result<()> {
        // Run all of the startup scripts
        for (name, command) in self.startup_commands.into_iter() {
            let log_dir = self.log_dir.clone();
            let db_connection = self.db_connection.clone();
            thread::spawn(move || loop {
                Self::exec_command(&db_connection, &log_dir, name.as_str(), command.as_str())
                    .unwrap();

                // Re-run the command after a few seconds
                thread::sleep(std::time::Duration::from_secs(3));
            });
        }

        // Run the scheduled scripts
        loop {
            self.scheduler.tick();
            thread::sleep(self.scheduler.time_till_next_job());
        }
    }

    // Helper to validate the command name
    fn validate_name(name: &str) -> bool {
        lazy_static::lazy_static! {
            static ref RE: Regex =
                Regex::new(r"^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$").unwrap();
        }
        RE.is_match(name)
    }

    // Helper to execute the specified command
    fn exec_command(
        db_connection: &Arc<Mutex<SqliteConnection>>,
        log_dir: &Path,
        name: &str,
        command: &str,
    ) -> Result<()> {
        let start_time = chrono::Local::now();
        let formatted_start_time = start_time.to_rfc3339();

        println!("{formatted_start_time} Running {name}: {command}");

        // Record the run in the database
        let connection = db_connection.lock().unwrap();
        diesel::insert_into(run::table)
            .values(run::dsl::name.eq(name))
            .execute(&*connection)
            .context("Error saving run to database")?;
        let run_id = diesel::select(last_insert_rowid)
            .get_result::<i32>(&*connection)
            .context("Error getting inserted run id from database")?;
        drop(connection);

        // Open the log file, creating the directory if necessary
        fs::create_dir_all(log_dir)
            .with_context(|| format!("Failed to create log dir {log_dir:?}"))?;
        let mut log_path = log_dir.join(name);
        log_path.set_extension("log");
        let mut log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path.clone())
            .with_context(|| format!("Failed to open log file {log_path:?}"))?;

        // Write the log file header for this execution
        let divider: &'static str = "----------------------------------------------------------------------------------------------------";
        log_file.write_all(format!("{formatted_start_time}\n{divider}\n").as_bytes())?;

        // Run the command
        let clone_log_file = || log_file.try_clone().context("Failed to clone log file");
        let status = process::Command::new("sh")
            .args(["-c", command])
            .stdin(process::Stdio::null())
            .stdout(clone_log_file()?)
            .stderr(clone_log_file()?)
            .status()
            .with_context(|| format!("Failed to run command {command}"))?;

        // Write the log file footer that contains the execution status
        let status_code = match status.code() {
            Some(code) => code.to_string(),
            None => "unknown".to_string(),
        };
        log_file.write_all(format!("{divider}\nStatus: {status_code}\n\n").as_bytes())?;

        // Update the run status code in the database
        if let Some(code) = status.code() {
            diesel::update(run::table.find(run_id))
                .set(run::dsl::status_code.eq(code))
                .execute(&*db_connection.lock().unwrap())
                .context("Error updating run status in the database")?;
        }

        Ok(())
    }
}
