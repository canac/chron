use crate::database::Database;
use crate::scheduled_job::ScheduledJob;
use crate::terminate_controller::TerminateController;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use cron::Schedule;
use lazy_static::lazy_static;
use log::{debug, info, warn};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread;
use std::time::Duration;

pub type ChronServiceLock = Arc<RwLock<ChronService>>;
pub type JobLock = Arc<RwLock<Job>>;
pub type DatabaseLock = Arc<Mutex<Database>>;

pub enum JobType {
    Startup,
    Scheduled(Box<ScheduledJob>),
}

pub struct RetryConfig {
    pub failures: bool,
    pub successes: bool,
    pub limit: Option<u64>,
    pub delay: Duration,
}

impl RetryConfig {
    // Determine the number of times that a job should be retried
    fn get_retry_count(&self) -> u64 {
        self.limit.unwrap_or(std::u64::MAX)
    }

    // Determine whether a command with a certain status should be retried
    fn should_retry(&self, exec_status: ExecStatus) -> bool {
        match exec_status {
            ExecStatus::Aborted | ExecStatus::Failure => self.failures,
            ExecStatus::Success => self.successes,
        }
    }
}

pub struct StartupJobOptions {
    pub keep_alive: RetryConfig,
}

pub struct ScheduledJobOptions {
    // Maximum number of missed runs to make up
    pub make_up_missed_runs: u64,
    pub retry: RetryConfig,
}

pub struct Job {
    pub name: String,
    pub command: String,
    pub log_path: PathBuf,
    pub process: Option<Child>,
    pub job_type: JobType,
}

enum ExecStatus {
    Success,
    Failure,
    Aborted,
}

pub struct ChronService {
    log_dir: PathBuf,
    db: DatabaseLock,
    jobs: HashMap<String, JobLock>,
    terminate_controller: TerminateController,
    me: Weak<RwLock<ChronService>>,
}

impl ChronService {
    // Create a new ChronService instance
    pub fn new(chron_dir: &Path) -> Result<ChronServiceLock> {
        let db = Database::new(chron_dir)?;
        Ok(Arc::new_cyclic(|me| {
            RwLock::new(ChronService {
                log_dir: chron_dir.join("logs"),
                db: Arc::new(Mutex::new(db)),
                jobs: HashMap::new(),
                terminate_controller: TerminateController::new(),
                me: me.clone(),
            })
        }))
    }

    // Return the service's database connection
    pub fn get_db(&self) -> DatabaseLock {
        self.db.clone()
    }

    // Lookup a job by name
    pub fn get_job(&self, name: &String) -> Option<&JobLock> {
        self.jobs.get(name)
    }

    // Return an iterator of the jobs
    pub fn get_jobs_iter(&self) -> impl Iterator<Item = (&String, &JobLock)> {
        self.jobs.iter()
    }

    // Return the Arc<RwLock> of this ChronService
    pub fn get_me(&self) -> Result<ChronServiceLock> {
        self.me
            .upgrade()
            .ok_or_else(|| anyhow!("Self has been destructed"))
    }

    // Add a new job to be run on startup
    pub fn startup(&mut self, name: &str, command: &str, options: StartupJobOptions) -> Result<()> {
        if !Self::validate_name(name) {
            bail!("Invalid job name {name}")
        }

        if self.jobs.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let job = Arc::new(RwLock::new(Job {
            name: name.to_string(),
            command: command.to_string(),
            log_path: self.calculate_log_path(name),
            process: None,
            job_type: JobType::Startup,
        }));
        self.jobs.insert(name.to_string(), job.clone());

        let me = self.get_me()?;
        let terminate_controller = self.terminate_controller.clone();
        thread::spawn(move || {
            // Loop up to the configured number of retries
            for _ in 0..options.keep_alive.get_retry_count() {
                // Stop executing if a terminate was requested
                if terminate_controller.is_terminated() {
                    break;
                }

                let mut job_guard = job.write().unwrap();
                if matches!(&mut job_guard.job_type, JobType::Startup) {
                    drop(job_guard);
                    let status = Self::exec_command(&me, &job, &terminate_controller).unwrap();
                    if !&options.keep_alive.should_retry(status) {
                        break;
                    }

                    // Re-run the job after the configured delay
                    Self::sleep_duration(options.keep_alive.delay).unwrap();
                }
            }
        });

        Ok(())
    }

    // Add a new job to be run on the given schedule
    pub fn schedule<'cmd>(
        &mut self,
        name: &str,
        schedule_expression: &str,
        command: &'cmd str,
        options: ScheduledJobOptions,
    ) -> Result<()> {
        if !Self::validate_name(name) {
            bail!("Invalid job name {name}")
        }

        if self.jobs.contains_key(name) {
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
        let job = Arc::new(RwLock::new(Job {
            name: name.to_string(),
            command: command.to_string(),
            log_path: self.calculate_log_path(name),
            process: None,
            job_type: JobType::Scheduled(Box::new(ScheduledJob::new(
                schedule.clone(),
                last_run_time,
            ))),
        }));
        self.jobs.insert(name.to_string(), job.clone());

        let me = self.get_me()?;
        let terminate_controller = self.terminate_controller.clone();
        let name = name.to_string();
        thread::spawn(move || {
            if let Some(last_run) = last_run_time {
                // Count the number of missed runs
                let now = Utc::now();
                let missed_runs = schedule
                    .after(&last_run)
                    .enumerate()
                    .take_while(|(count, run)| {
                        run <= &now && (*count as u64) < options.make_up_missed_runs
                    })
                    .count();

                // Make up the missed runs
                if missed_runs > 0 {
                    for run in 1..=missed_runs {
                        // Stop executing if a terminate was requested
                        if terminate_controller.is_terminated() {
                            break;
                        }

                        debug!("{name}: making up missed run {run} of {missed_runs}");
                        Self::exec_command(&me, &job, &terminate_controller).unwrap();
                    }
                }
            }

            loop {
                // Stop executing if a terminate was requested
                if terminate_controller.is_terminated() {
                    break;
                }

                let mut job_guard = job.write().unwrap();
                if let JobType::Scheduled(scheduled_job) = &mut job_guard.job_type {
                    let should_run = scheduled_job.tick();
                    let next_run = scheduled_job.next_run();
                    drop(job_guard);

                    if should_run {
                        // Run the command up to the configured number of times
                        for attempt in 0..=RetryConfig::get_retry_count(&options.retry) {
                            // Stop executing if a terminate was requested
                            if terminate_controller.is_terminated() {
                                break;
                            }

                            if attempt != 0 {
                                debug!("{name}: retry attempt {attempt}");
                            }
                            let status =
                                Self::exec_command(&me, &job, &terminate_controller).unwrap();
                            if !RetryConfig::should_retry(&options.retry, status) {
                                break;
                            }
                        }
                    }

                    // Wait until the next run before ticking again
                    match next_run {
                        Some(next_run) => Self::sleep_until(next_run),
                        None => break,
                    };
                }
            }
        });

        Ok(())
    }

    // Delete and stop all previously registered jobs
    pub fn reset(&mut self) -> Result<()> {
        self.jobs.clear();
        self.terminate_controller.terminate();
        self.terminate_controller = TerminateController::new();
        Ok(())
    }

    // Helper to validate the job name
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
        chron_lock: &ChronServiceLock,
        job_lock: &JobLock,
        terminate_controller: &TerminateController,
    ) -> Result<ExecStatus> {
        // Don't run the job at all if it is supposed to be terminated
        if terminate_controller.is_terminated() {
            return Ok(ExecStatus::Aborted);
        }

        let start_time = chrono::Local::now();
        let formatted_start_time = start_time.to_rfc3339();

        let mut job = job_lock.write().unwrap();
        let name = job.name.clone();
        info!("{name}: running \"{}\"", job.command);

        // Record the run in the database
        let run = {
            let state_guard = chron_lock.read().unwrap();
            let mut db = state_guard.db.lock().unwrap();
            db.insert_run(&name)?
        };

        // Open the log file, creating the directory if necessary
        let log_dir = job
            .log_path
            .parent()
            .with_context(|| format!("Failed to get parent dir of log file {:?}", job.log_path))?;
        fs::create_dir_all(log_dir)
            .with_context(|| format!("Failed to create log dir {log_dir:?}"))?;
        let mut log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(job.log_path.clone())
            .with_context(|| format!("Failed to open log file {:?}", job.log_path))?;

        // Write the log file header for this execution
        lazy_static! {
            static ref DIVIDER: String = "-".repeat(80);
        }
        log_file.write_all(format!("{formatted_start_time}\n{}\n", *DIVIDER).as_bytes())?;

        // Run the command
        let clone_log_file = || log_file.try_clone().context("Failed to clone log file");
        let process = process::Command::new("sh")
            .args(["-c", &job.command])
            .stdin(process::Stdio::null())
            .stdout(clone_log_file()?)
            .stderr(clone_log_file()?)
            .spawn()
            .with_context(|| format!("Failed to run command {}", job.command))?;

        job.process = Some(process);
        drop(job);

        // Check the status every second until it exists without holding onto the child process lock
        let (status_code, status_code_str) = loop {
            let mut job = job_lock.write().unwrap();

            // Attempt to terminate the process if we got a terminate signal from the terminate controller
            if terminate_controller.is_terminated() {
                let result = job.process.as_mut().unwrap().kill();

                // If the result was an InvalidInput error, it is because the process already
                // terminated, so ignore that type of error
                match result {
                    Ok(_) => break (None, "terminated".to_string()),
                    Err(ref err) => {
                        // Propagate other errors
                        if !matches!(&err.kind(), std::io::ErrorKind::InvalidInput) {
                            result.with_context(|| {
                                format!("Failed to terminate command {}", job.command)
                            })?;
                        }
                    }
                }
            }

            // Try to read the process' exit status without blocking
            let process = job.process.as_mut().unwrap();
            let maybe_status = process.try_wait().with_context(|| {
                format!("Failed to get command status for command {}", job.command)
            })?;
            drop(job);
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

        let mut job = job_lock.write().unwrap();
        job.process = None;
        drop(job);

        // Write the log file footer that contains the execution status
        log_file.write_all(format!("{}\nStatus: {status_code_str}\n\n", *DIVIDER).as_bytes())?;

        if let Some(code) = status_code {
            // Update the run status code in the database
            chron_lock
                .read()
                .unwrap()
                .db
                .lock()
                .unwrap()
                .set_run_status_code(run.id, code)?;

            if code != 0 {
                warn!("{name}: failed with exit code {code}");

                if Command::new("mailbox")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .args([
                        "add",
                        format!("chron/error/{name}").as_str(),
                        format!("{name} failed with exit code {code}").as_str(),
                    ])
                    .spawn()
                    .is_err()
                {
                    warn!("Failed to write mailbox message");
                }
            }
        }

        Ok(match status_code {
            Some(code) if code != 0 => ExecStatus::Failure,
            _ => ExecStatus::Success,
        })
    }

    // Sleep for the specified duration, preventing oversleeping during hibernation
    fn sleep_duration(duration: Duration) -> Result<()> {
        Self::sleep_until(Utc::now() + chrono::Duration::from_std(duration)?);
        Ok(())
    }

    // Sleep until the specified timestamp, preventing oversleeping during hibernation
    fn sleep_until(timestamp: DateTime<Utc>) {
        let max_sleep = Duration::from_secs(60);
        // to_std returns Err if the duration is negative, in which case we
        // have hit the timestamp and cam stop looping
        while let Ok(duration) = timestamp.signed_duration_since(Utc::now()).to_std() {
            // Sleep for a maximum of one minute to prevent oversleeping when
            // the computer hibernates
            std::thread::sleep(std::cmp::min(duration, max_sleep))
        }
    }
}
