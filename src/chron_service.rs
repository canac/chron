use crate::chronfile::MakeupMissedRuns;
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

pub struct ScheduledJob {
    schedule: Schedule,
    last_tick: DateTime<Utc>,
}

impl ScheduledJob {
    // Create a new scheduled job
    pub fn new(schedule: Schedule, last_run: Option<DateTime<Utc>>) -> Self {
        ScheduledJob {
            schedule,
            last_tick: last_run.unwrap_or_else(Utc::now),
        }
    }

    // Return the date of the next time that this scheduled job will run
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        self.schedule.after(&self.last_tick).next()
    }

    // Determine whether the job should be run
    pub fn tick(&mut self) -> bool {
        let now = Utc::now();
        let should_run = self.next_run().map_or(false, |next_run| next_run <= now);
        self.last_tick = now;
        should_run
    }
}

pub type ChronServiceLock = Arc<RwLock<ChronService>>;
pub type JobLock = Arc<RwLock<Job>>;
pub type DatabaseLock = Arc<Mutex<Database>>;

pub enum JobType {
    Startup,
    Scheduled(Box<ScheduledJob>),
}

pub struct Job {
    pub name: String,
    pub command: String,
    pub log_path: PathBuf,
    pub process: Option<Child>,
    pub job_type: JobType,
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
    pub fn startup(&mut self, name: &str, command: &str) -> Result<()> {
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
        thread::spawn(move || loop {
            // Stop executing if a terminate was requested
            if terminate_controller.is_terminated() {
                break;
            }

            let mut job_guard = job.write().unwrap();
            if matches!(&mut job_guard.job_type, JobType::Startup) {
                drop(job_guard);
                Self::exec_command(&me, &job, &terminate_controller).unwrap();

                // Re-run the job after a few seconds
                thread::sleep(Duration::from_secs(3));
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
        makeup_missed_runs: MakeupMissedRuns,
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
                let max_missed_runs = match makeup_missed_runs {
                    MakeupMissedRuns::All => None,
                    MakeupMissedRuns::Count(count) => Some(count),
                };
                let missed_runs = schedule
                    .after(&last_run)
                    .enumerate()
                    .take_while(|(count, run)| {
                        run <= &now && max_missed_runs.map_or(true, |max| (*count as u64) < max)
                    })
                    .count();

                // Make up the missed runs
                if missed_runs > 0 {
                    eprintln!("Making up {} missed runs for {}", missed_runs, name);
                    for _ in 0..missed_runs {
                        // Stop executing if a terminate was requested
                        if terminate_controller.is_terminated() {
                            break;
                        }

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
                        Self::exec_command(&me, &job, &terminate_controller).unwrap();
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
    ) -> Result<()> {
        // Don't run the job at all if it is supposed to be terminated
        if terminate_controller.is_terminated() {
            return Ok(());
        }

        let start_time = chrono::Local::now();
        let formatted_start_time = start_time.to_rfc3339();

        let mut job = job_lock.write().unwrap();
        println!(
            "{formatted_start_time} Running {}: {}",
            job.name, job.command
        );

        // Record the run in the database
        let run_id = {
            let state_guard = chron_lock.read().unwrap();
            let db = state_guard.db.lock().unwrap();
            db.insert_run(&job.name)?
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
