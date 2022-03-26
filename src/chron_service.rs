use crate::database::Database;
use crate::http_server;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use cron::Schedule;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Child};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct ScheduledCommand {
    schedule: Schedule,
    last_tick: Option<DateTime<Utc>>,
}

impl ScheduledCommand {
    // Return the date of the next time that this scheduled command will run
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        let now = Utc::now();
        self.schedule.after(&now).next()
    }

    fn tick(&mut self) -> bool {
        let now = Utc::now();
        let should_run = match self.last_tick {
            None => false,
            Some(last_tick) => {
                if let Some(event) = self.schedule.after(&last_tick).next() {
                    event <= now
                } else {
                    false
                }
            }
        };
        self.last_tick = Some(now);
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

#[derive(Clone)]
pub struct ThreadState {
    pub db: Arc<Mutex<Database>>,
    pub commands: HashMap<String, Arc<Mutex<Command>>>,
}

pub struct ChronService {
    log_dir: PathBuf,
    state: ThreadState,
}

impl ChronService {
    // Create a new ChronService instance
    pub fn new(chron_dir: &Path) -> Result<Self> {
        let db = Database::new(chron_dir)?;
        let thread_state = ThreadState {
            db: Arc::new(Mutex::new(db)),
            commands: HashMap::new(),
        };
        Ok(ChronService {
            log_dir: chron_dir.join("logs"),
            state: thread_state,
        })
    }

    // Add a new command to be run on startup
    pub fn startup(&mut self, name: &str, command: &str) -> Result<()> {
        if !Self::validate_name(name) {
            bail!("Invalid command name {name}")
        }

        if self.state.commands.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let command = Command {
            name: name.to_string(),
            command: command.to_string(),
            log_path: self.calculate_log_path(name),
            process: None,
            command_type: CommandType::Startup,
        };
        self.state
            .commands
            .insert(name.to_string(), Arc::new(Mutex::new(command)));

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

        if self.state.commands.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let schedule = Schedule::from_str(schedule_expression).with_context(|| {
            format!("Failed to parse schedule expression {schedule_expression}")
        })?;
        let command = Command {
            name: name.to_string(),
            command: command.to_string(),
            log_path: self.calculate_log_path(name),
            process: None,
            command_type: CommandType::Scheduled(Box::new(ScheduledCommand {
                schedule,
                last_tick: None,
            })),
        };
        self.state
            .commands
            .insert(name.to_string(), Arc::new(Mutex::new(command)));

        Ok(())
    }

    pub async fn start_server(&self) -> Result<(), std::io::Error> {
        http_server::start_server(self.state.clone()).await
    }

    // Start the chron service, running all scripts on their specified interval
    // Note that this function starts an infinite loop and will never return
    pub fn run(&self) -> Result<()> {
        // Run all of the startup scripts
        for command in self.state.commands.values() {
            let thread_state = self.state.clone();
            let command = command.clone();
            thread::spawn(move || loop {
                let mut command_guard = command.lock().unwrap();
                match &mut command_guard.command_type {
                    CommandType::Startup => {
                        drop(command_guard);
                        Self::exec_command(&thread_state, &command).unwrap();

                        // Re-run the command after a few seconds
                        thread::sleep(Duration::from_secs(3));
                    }
                    CommandType::Scheduled(scheduled_command) => {
                        let should_run = scheduled_command.tick();
                        drop(command_guard);
                        if should_run {
                            Self::exec_command(&thread_state, &command).unwrap();
                            thread::sleep(Duration::from_millis(500));
                        }
                    }
                }
            });
        }

        Ok(())
    }

    // Kill a process by its name
    pub fn kill_by_name(&self, name: &str) -> Result<()> {
        let mut command = self
            .state
            .commands
            .get(&name.to_string())
            .ok_or_else(|| anyhow!("No process with name {name}"))?
            .lock()
            .unwrap();
        if let Some(process) = command.process.as_mut() {
            process.kill()?;
        }
        Ok(())
    }

    // Lookup the process id of a job by its name
    pub fn pid_from_name(&self, name: &str) -> Option<u32> {
        self.state
            .commands
            .get(&name.to_string())
            .and_then(|child| {
                let child = child.lock().unwrap();
                child.process.as_ref().map(|process| process.id())
            })
    }

    // Helper to validate the command name
    fn validate_name(name: &str) -> bool {
        lazy_static::lazy_static! {
            static ref RE: Regex =
                Regex::new(r"^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$").unwrap();
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
    fn exec_command(thread_state: &ThreadState, command_mutex: &Arc<Mutex<Command>>) -> Result<()> {
        let start_time = chrono::Local::now();
        let formatted_start_time = start_time.to_rfc3339();

        let mut command = command_mutex.lock().unwrap();
        println!(
            "{formatted_start_time} Running {}: {}",
            command.name, command.command
        );

        // Record the run in the database
        let db = thread_state.db.lock().unwrap();
        let run_id = db.insert_run(&command.name)?;
        drop(db);

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
        let divider: &'static str = "----------------------------------------------------------------------------------------------------";
        log_file.write_all(format!("{formatted_start_time}\n{divider}\n").as_bytes())?;

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

        // Check the status every second until it exists without holding onto the child process mutex
        let status = loop {
            let mut command = command_mutex.lock().unwrap();
            let maybe_status = command
                .process
                .as_mut()
                .unwrap()
                .try_wait()
                .with_context(|| {
                    format!(
                        "Failed to get command status for command {}",
                        command.command
                    )
                })?;
            drop(command);
            match maybe_status {
                None => std::thread::sleep(Duration::from_millis(1000)),
                Some(status) => {
                    break status;
                }
            }
        };

        let mut command = command_mutex.lock().unwrap();
        command.process = None;
        drop(command);

        // Write the log file footer that contains the execution status
        let status_code = match status.code() {
            Some(code) => code.to_string(),
            None => "unknown".to_string(),
        };
        log_file.write_all(format!("{divider}\nStatus: {status_code}\n\n").as_bytes())?;

        // Update the run status code in the database
        if let Some(code) = status.code() {
            thread_state
                .db
                .lock()
                .unwrap()
                .set_run_status_code(run_id, code)?;
        }

        Ok(())
    }
}
