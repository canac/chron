use crate::database::Database;
use crate::terminate_controller::TerminateController;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use cron::Schedule;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Child};
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread;
use std::time::Duration;

pub struct ScheduledCommand {
    schedule: Schedule,
    last_tick: DateTime<Utc>,
}

impl ScheduledCommand {
    // Create a new scheduled command
    pub fn new(schedule: Schedule, last_run: Option<DateTime<Utc>>) -> Self {
        ScheduledCommand {
            schedule,
            last_tick: last_run.unwrap_or_else(Utc::now),
        }
    }

    // Return the date of the next time that this scheduled command will run
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        self.schedule.after(&self.last_tick).next()
    }

    // Determine whether the command should be run
    pub fn tick(&mut self) -> bool {
        let now = Utc::now();
        let should_run = self.next_run().map_or(false, |next_run| next_run <= now);
        self.last_tick = now;
        should_run
    }
}

pub enum CommandType {
    Startup,
    Scheduled(Box<ScheduledCommand>),
}

pub struct Command {
    pub name: String,
    pub command: String,
    pub log_path: PathBuf,
    pub process: Option<Child>,
    pub command_type: CommandType,
}

pub struct ChronService {
    pub log_dir: PathBuf,
    pub db: Arc<Mutex<Database>>,
    pub commands: HashMap<String, Arc<RwLock<Command>>>,
    pub terminate_controller: Option<TerminateController>,
    pub me: Weak<RwLock<ChronService>>,
}

impl ChronService {
    // Create a new ChronService instance
    pub fn new(chron_dir: &Path) -> Result<Arc<RwLock<Self>>> {
        let db = Database::new(chron_dir)?;
        Ok(Arc::new_cyclic(|me| {
            RwLock::new(ChronService {
                log_dir: chron_dir.join("logs"),
                db: Arc::new(Mutex::new(db)),
                commands: HashMap::new(),
                terminate_controller: None,
                me: me.clone(),
            })
        }))
    }

    // Return the Arc<RwLock> of this ChronService
    pub fn get_me(&self) -> Result<Arc<RwLock<Self>>> {
        self.me
            .upgrade()
            .ok_or_else(|| anyhow!("Self has been destructed"))
    }

    // Add a new command to be run on startup
    pub fn startup(&mut self, name: &str, command: &str) -> Result<()> {
        if !Self::validate_name(name) {
            bail!("Invalid command name {name}")
        }

        if self.commands.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let command = Command {
            name: name.to_string(),
            command: command.to_string(),
            log_path: self.calculate_log_path(name),
            process: None,
            command_type: CommandType::Startup,
        };
        self.commands
            .insert(name.to_string(), Arc::new(RwLock::new(command)));

        Ok(())
    }

    // Add a new command to be run on the given schedule
    pub fn schedule<'cmd>(
        &mut self,
        name: &str,
        schedule_expression: &str,
        command: &'cmd str,
    ) -> Result<()> {
        if !Self::validate_name(name) {
            bail!("Invalid command name {name}")
        }

        if self.commands.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let last_run_time = self
            .db
            .lock()
            .unwrap()
            .get_last_run_time(name)?
            .map(|last_run_time| Utc.from_utc_datetime(&last_run_time));

        let schedule = Schedule::from_str(schedule_expression).with_context(|| {
            format!("Failed to parse schedule expression {schedule_expression}")
        })?;
        let command = Command {
            name: name.to_string(),
            command: command.to_string(),
            log_path: self.calculate_log_path(name),
            process: None,
            command_type: CommandType::Scheduled(Box::new(ScheduledCommand::new(
                schedule,
                last_run_time,
            ))),
        };
        self.commands
            .insert(name.to_string(), Arc::new(RwLock::new(command)));

        Ok(())
    }

    // Delete all previously registered commands
    pub fn reset(&mut self) -> Result<()> {
        self.commands.clear();
        self.stop()
    }

    // Start the chron service, running all scripts on their specified interval
    pub fn start(&mut self) -> Result<()> {
        self.stop()?;

        let terminate_controller = TerminateController::new();
        self.terminate_controller = Some(terminate_controller.clone());

        for command in self.commands.values() {
            let me = self.get_me()?;
            let command = command.clone();
            let terminate_controller = terminate_controller.clone();

            // Spawn a new thread for each command
            thread::spawn(move || loop {
                // If we got a terminate message, break out of the loop
                if terminate_controller.is_terminated() {
                    break;
                }

                let mut command_guard = command.write().unwrap();
                match &mut command_guard.command_type {
                    CommandType::Startup => {
                        drop(command_guard);
                        Self::exec_command(&me, &command, &terminate_controller).unwrap();

                        // Re-run the command after a few seconds
                        thread::sleep(Duration::from_secs(3));
                    }
                    CommandType::Scheduled(scheduled_command) => {
                        let should_run = scheduled_command.tick();
                        let next_run = scheduled_command.next_run();

                        drop(command_guard);
                        if should_run {
                            Self::exec_command(&me, &command, &terminate_controller).unwrap();
                        }

                        // Wait until the next run before ticking again
                        let sleep_duration = next_run
                            .and_then(|next_run| {
                                next_run.signed_duration_since(Utc::now()).to_std().ok()
                            })
                            .unwrap_or_else(|| Duration::from_millis(500));
                        // Sleep for a maximum of one minute to avoid oversleeping when the computer hibernates
                        thread::sleep(std::cmp::min(sleep_duration, Duration::from_secs(60)));
                    }
                }
            });
        }

        Ok(())
    }

    // Stop the chron service and all running commands
    // If the chron service hasn't been started, then this method does nothing
    pub fn stop(&mut self) -> Result<()> {
        if let Some(terminate_controller) = &self.terminate_controller {
            terminate_controller.terminate();
            self.terminate_controller = None;
        }

        Ok(())
    }

    // Helper to validate the command name
    fn validate_name(name: &str) -> bool {
        lazy_static! {
            static ref RE: Regex = Regex::new(r"^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$").unwrap();
        }
        RE.is_match(name)
    }

    // Helper to get the log file path for a command
    fn calculate_log_path(&self, name: &str) -> PathBuf {
        let mut log_path = self.log_dir.join(name);
        log_path.set_extension("log");
        log_path
    }

    // Helper to execute the specified command
    fn exec_command(
        chron_lock: &Arc<RwLock<ChronService>>,
        command_lock: &Arc<RwLock<Command>>,
        terminate_controller: &TerminateController,
    ) -> Result<()> {
        // Don't run the command at all if it is supposed to be terminated
        if terminate_controller.is_terminated() {
            return Ok(());
        }

        let start_time = chrono::Local::now();
        let formatted_start_time = start_time.to_rfc3339();

        let mut command = command_lock.write().unwrap();
        println!(
            "{formatted_start_time} Running {}: {}",
            command.name, command.command
        );

        // Record the run in the database
        let run_id = {
            let state_guard = chron_lock.read().unwrap();
            let db = state_guard.db.lock().unwrap();
            db.insert_run(&command.name)?
        };

        // Open the log file, creating the directory if necessary
        let log_dir = command.log_path.parent().with_context(|| {
            format!(
                "Failed to get parent dir of log file {:?}",
                command.log_path
            )
        })?;
        fs::create_dir_all(log_dir)
            .with_context(|| format!("Failed to create log dir {log_dir:?}"))?;
        let mut log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(command.log_path.clone())
            .with_context(|| format!("Failed to open log file {:?}", command.log_path))?;

        // Write the log file header for this execution
        lazy_static! {
            static ref DIVIDER: String = "-".repeat(80);
        }
        log_file.write_all(format!("{formatted_start_time}\n{}\n", *DIVIDER).as_bytes())?;

        // Run the command
        let clone_log_file = || log_file.try_clone().context("Failed to clone log file");
        let process = process::Command::new("sh")
            .args(["-c", &command.command])
            .stdin(process::Stdio::null())
            .stdout(clone_log_file()?)
            .stderr(clone_log_file()?)
            .spawn()
            .with_context(|| format!("Failed to run command {}", command.command))?;

        command.process = Some(process);
        drop(command);

        // Check the status every second until it exists without holding onto the child process lock
        let (status_code, status_code_str) = loop {
            let mut command = command_lock.write().unwrap();

            // Attempt to terminate the process if we got a terminate signal from the terminate controller
            if terminate_controller.is_terminated() {
                let result = command.process.as_mut().unwrap().kill();

                // If the result was an InvalidInput error, it is because the process already
                // terminated, so ignore that type of error
                match result {
                    Ok(_) => break (None, "terminated".to_string()),
                    Err(ref err) => {
                        // Propagate other errors
                        if !matches!(&err.kind(), std::io::ErrorKind::InvalidInput) {
                            result.with_context(|| {
                                format!("Failed to terminate command {}", command.command)
                            })?;
                        }
                    }
                }
            }

            // Try to read the process' exit status without blocking
            let process = command.process.as_mut().unwrap();
            let maybe_status = process.try_wait().with_context(|| {
                format!(
                    "Failed to get command status for command {}",
                    command.command
                )
            })?;
            drop(command);
            match maybe_status {
                // Wait a second, then loop again
                None => std::thread::sleep(Duration::from_millis(1000)),
                Some(status) => {
                    break match status.code() {
                        Some(code) => (Some(code), code.to_string()),
                        None => (None, "unknown".to_string()),
                    };
                }
            }
        };

        let mut command = command_lock.write().unwrap();
        command.process = None;
        drop(command);

        // Write the log file footer that contains the execution status
        log_file.write_all(format!("{}\nStatus: {status_code_str}\n\n", *DIVIDER).as_bytes())?;

        // Update the run status code in the database
        if let Some(code) = status_code {
            chron_lock
                .read()
                .unwrap()
                .db
                .lock()
                .unwrap()
                .set_run_status_code(run_id, code)?;
        }

        Ok(())
    }
}
