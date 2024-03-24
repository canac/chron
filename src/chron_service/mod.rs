mod exec;
mod scheduled_job;
mod sleep;
mod terminate_controller;

use self::exec::{exec_command, ExecStatus};
use self::scheduled_job::ScheduledJob;
use self::sleep::sleep_until;
use self::terminate_controller::TerminateController;
use crate::chronfile::Chronfile;
use crate::database::Database;
use anyhow::{anyhow, bail, Context, Result};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use cron::Schedule;
use lazy_static::lazy_static;
use log::debug;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread;
use std::time::Duration;

pub type ChronServiceLock = Arc<RwLock<ChronService>>;
pub type DatabaseLock = Arc<Mutex<Database>>;

pub enum JobType {
    Startup {
        options: Arc<StartupJobOptions>,
    },
    Scheduled {
        options: Arc<ScheduledJobOptions>,
        scheduled_job: RwLock<Box<ScheduledJob>>,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub struct RetryConfig {
    pub failures: bool,
    pub successes: bool,
    pub limit: Option<usize>,
    pub delay: Option<Duration>,
}

impl RetryConfig {
    // Determine whether a command with a certain status and a certain number
    // of previous attempts should be retried
    fn should_retry(&self, exec_status: ExecStatus, attempt: usize) -> bool {
        (match exec_status {
            ExecStatus::Aborted | ExecStatus::Failure => self.failures,
            ExecStatus::Success => self.successes,
        } && match self.limit {
            None => true,
            Some(limit) => attempt < limit,
        })
    }
}

#[derive(PartialEq)]
pub struct StartupJobOptions {
    pub keep_alive: RetryConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakeUpMissedRuns {
    Limited(usize),
    Unlimited,
}

#[derive(PartialEq)]
pub struct ScheduledJobOptions {
    // Maximum number of missed runs to make up
    pub make_up_missed_runs: MakeUpMissedRuns,
    pub retry: RetryConfig,
}

pub struct Job {
    pub name: String,
    pub command: String,
    pub shell: String,
    pub log_path: PathBuf,
    pub process: RwLock<Option<Child>>,
    pub terminate_controller: TerminateController,
    #[allow(clippy::struct_field_names)]
    pub job_type: JobType,
}

pub struct ChronService {
    log_dir: PathBuf,
    db: DatabaseLock,
    jobs: HashMap<String, Arc<Job>>,
    default_shell: String,
    shell: Option<String>,
    me: Weak<RwLock<ChronService>>,
}

impl ChronService {
    // Create a new ChronService instance
    pub fn new(chron_dir: &Path) -> Result<ChronServiceLock> {
        // Make sure that the chron directory exists
        std::fs::create_dir_all(chron_dir)?;

        let db = Database::new(chron_dir)?;
        Ok(Arc::new_cyclic(|me| {
            RwLock::new(ChronService {
                log_dir: chron_dir.join("logs"),
                db: Arc::new(Mutex::new(db)),
                jobs: HashMap::new(),
                default_shell: Self::get_user_shell().unwrap(),
                shell: None,
                me: me.clone(),
            })
        }))
    }

    // Return the service's database connection
    pub fn get_db(&self) -> DatabaseLock {
        self.db.clone()
    }

    // Lookup a job by name
    pub fn get_job(&self, name: &String) -> Option<&Arc<Job>> {
        self.jobs.get(name)
    }

    // Return an iterator of the jobs
    pub fn get_jobs_iter(&self) -> impl Iterator<Item = (&String, &Arc<Job>)> {
        self.jobs.iter()
    }

    // Start or start the chron service using the jobs defined in the provided chronfile
    pub fn start(&mut self, chronfile: Chronfile) -> Result<()> {
        let same_shell = self.shell == chronfile.config.shell;
        self.shell = chronfile.config.shell;

        let mut existing_jobs = std::mem::take(&mut self.jobs);

        for (name, job) in chronfile.startup_jobs {
            if job.disabled {
                continue;
            }

            if let Some(existing_job) = existing_jobs.get(&name) {
                // Reuse the job if the command, shell, and options are the same
                if existing_job.command == job.command
                    && same_shell
                    && matches!(&existing_job.job_type, JobType::Startup { options } if **options == job.get_options())
                {
                    debug!("Reusing existing startup job {name}");
                    self.jobs
                        .insert(name.clone(), existing_jobs.remove(&name).unwrap());
                    continue;
                }
            }

            debug!("Registering new startup job {name}");
            self.startup(&name, &job.command, job.get_options())?;
        }

        for (name, job) in chronfile.scheduled_jobs {
            if job.disabled {
                continue;
            }

            if let Some(existing_job) = existing_jobs.get(&name) {
                // Reuse the job if the command, shell, options, and schedule are the same
                if existing_job.command == job.command
                    && same_shell
                    && matches!(&existing_job.job_type, JobType::Scheduled { options, scheduled_job } if **options == job.get_options() && scheduled_job.read().unwrap().get_schedule() == job.schedule)
                {
                    debug!("Reusing existing scheduled job {name}");
                    self.jobs
                        .insert(name.clone(), existing_jobs.remove(&name).unwrap());
                    continue;
                }
            }

            debug!("Registering new scheduled job {name}");
            self.schedule(&name, &job.schedule, &job.command, job.get_options())?;
        }

        // Terminate all existing jobs that weren't reused
        for (name, existing_job) in existing_jobs {
            debug!("Terminating job {name}");
            existing_job.terminate_controller.terminate();
        }

        Ok(())
    }

    // Add a new job to be run on startup
    fn startup(&mut self, name: &str, command: &str, options: StartupJobOptions) -> Result<()> {
        Self::validate_name(name)?;
        if self.jobs.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let options = Arc::new(options);
        let job = Arc::new(Job {
            name: name.to_string(),
            command: command.to_string(),
            shell: self.get_shell(),
            log_path: self.calculate_log_path(name),
            process: RwLock::new(None),
            terminate_controller: TerminateController::new(),
            job_type: JobType::Startup {
                options: options.clone(),
            },
        });
        self.jobs.insert(name.to_string(), job.clone());

        let me = self.get_me()?;
        thread::spawn(move || {
            exec_command(&me, &job, &options.keep_alive).unwrap();
        });

        Ok(())
    }

    // Add a new job to be run on the given schedule
    #[allow(clippy::too_many_lines)]
    fn schedule(
        &mut self,
        name: &str,
        schedule_expression: &str,
        command: &str,
        options: ScheduledJobOptions,
    ) -> Result<()> {
        Self::validate_name(name)?;
        if self.jobs.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        // Resume the job scheduler from the checkpoint time, which is the
        // scheduled time of the last successful (i.e. not retried) run
        let resume_time = self.db.lock().unwrap().get_checkpoint(name)?;

        let schedule = Schedule::from_str(schedule_expression).with_context(|| {
            format!("Failed to parse schedule expression {schedule_expression}")
        })?;
        let scheduled_job = ScheduledJob::new(schedule, resume_time);
        if resume_time.is_none() {
            // If the job doesn't have a checkpoint, estimate one based on the
            // schedule extrapolated backwards in time. This is to prevent jobs
            // with a long period from never running if chron isn't running
            // when they are scheduled to run. For example, assume a job is
            // scheduled to run on Sundays at midnight, and chron is stopped on
            // Saturday and restarted on Monday. When it restarts, because
            // there isn't a checkpoint, it won't be able to resume and will
            // schedule the backup for the next Sunday. Eagerly saving a
            // checkpoint prevents this problem.
            let start_time = scheduled_job
                .prev_run()
                .ok_or_else(|| anyhow!("Failed to calculate start time"))?;
            self.db.lock().unwrap().set_checkpoint(name, start_time)?;
        }
        let job = Arc::new(Job {
            name: name.to_string(),
            command: command.to_string(),
            shell: self.get_shell(),
            log_path: self.calculate_log_path(name),
            process: RwLock::new(None),
            terminate_controller: TerminateController::new(),
            job_type: JobType::Scheduled {
                options: Arc::new(options),
                scheduled_job: RwLock::new(Box::new(scheduled_job)),
            },
        });
        self.jobs.insert(name.to_string(), job.clone());

        let me = self.get_me()?;
        let name = name.to_string();
        thread::spawn(move || {
            for iteration in 0.. {
                // Stop executing if a terminate was requested
                if job.terminate_controller.is_terminated() {
                    break;
                }

                let JobType::Scheduled {
                    ref options,
                    ref scheduled_job,
                } = job.job_type
                else {
                    continue;
                };
                // On the first tick, all of the runs are makeup runs, but
                // on subsequent ticks, the last run is a regular run and
                // the rest are makeup runs
                let has_regular_run = iteration != 0;
                let num_regular_runs = usize::from(has_regular_run);
                let max_runs = match options.make_up_missed_runs {
                    MakeUpMissedRuns::Unlimited => None,
                    MakeUpMissedRuns::Limited(limit) => Some(limit + num_regular_runs),
                };

                // Get the elapsed runs
                let mut job_guard = scheduled_job.write().unwrap();
                let runs = job_guard.tick(max_runs);

                // Retry delay defaults to one sixth of the job's period
                let retry_delay = options
                    .retry
                    .delay
                    .unwrap_or_else(|| job_guard.get_current_period().unwrap() / 6);

                let next_run = job_guard.next_run();
                drop(job_guard);

                // Execute the most recent runs
                let run_count = runs.len();
                for (run, scheduled_timestamp) in runs.into_iter().enumerate() {
                    // Stop executing if a terminate was requested
                    if job.terminate_controller.is_terminated() {
                        break;
                    }

                    let is_makeup_run = if has_regular_run {
                        // The last run is a regular run, not a makeup run
                        run != run_count - 1
                    } else {
                        true
                    };
                    if is_makeup_run {
                        debug!(
                            "{name}: making up missed run {} of {}",
                            run + 1,
                            run_count - num_regular_runs
                        );
                    }

                    let late = HumanTime::from(scheduled_timestamp)
                        .to_text_en(Accuracy::Precise, Tense::Present);
                    debug!("{name}: scheduled for {scheduled_timestamp} ({late} late)");

                    let completed = exec_command(
                        &me,
                        &job,
                        &RetryConfig {
                            delay: Some(retry_delay),
                            ..options.retry
                        },
                    )
                    .unwrap();
                    if completed {
                        me.read()
                            .unwrap()
                            .db
                            .lock()
                            .unwrap()
                            .set_checkpoint(name.as_str(), scheduled_timestamp)
                            .unwrap();
                    }
                }

                // Wait until the next run before ticking again
                match next_run {
                    Some(next_run) => sleep_until(next_run),
                    None => break,
                };
            }
        });

        Ok(())
    }

    // Get the shell to execute commands with
    fn get_shell(&self) -> String {
        self.shell.as_ref().unwrap_or(&self.default_shell).clone()
    }

    // Return the Arc<RwLock> of this ChronService
    fn get_me(&self) -> Result<ChronServiceLock> {
        self.me
            .upgrade()
            .ok_or_else(|| anyhow!("Self has been destructed"))
    }

    // Helper to validate the job name
    fn validate_name(name: &str) -> Result<()> {
        lazy_static! {
            static ref RE: Regex = Regex::new(r"^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$").unwrap();
        }
        if RE.is_match(name) {
            Ok(())
        } else {
            bail!("Invalid job name {name}")
        }
    }

    // Helper to get the log file path for a command
    fn calculate_log_path(&self, name: &str) -> PathBuf {
        self.log_dir.join(format!("{name}.log"))
    }

    // Get the user's shell
    #[cfg(target_os = "windows")]
    fn get_user_shell() -> Result<String> {
        Ok("Invoke-Expression".to_string())
    }

    // Get the user's shell
    #[cfg(not(target_os = "windows"))]
    fn get_user_shell() -> Result<String> {
        std::env::var("SHELL").context("Couldn't get $SHELL environment variable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_retry() {
        let no_retries = RetryConfig {
            failures: false,
            successes: false,
            limit: None,
            delay: None,
        };
        let only_failures = RetryConfig {
            failures: true,
            successes: false,
            limit: None,
            delay: None,
        };
        let only_success = RetryConfig {
            failures: false,
            successes: true,
            limit: None,
            delay: None,
        };
        let limited_retries = RetryConfig {
            failures: true,
            successes: true,
            limit: Some(2),
            delay: None,
        };
        let unlimited_retries = RetryConfig {
            failures: true,
            successes: true,
            limit: None,
            delay: None,
        };

        assert!(!no_retries.should_retry(ExecStatus::Success, 0));
        assert!(!no_retries.should_retry(ExecStatus::Failure, 0));

        assert!(!only_failures.should_retry(ExecStatus::Success, 0));
        assert!(only_failures.should_retry(ExecStatus::Failure, 0));

        assert!(only_success.should_retry(ExecStatus::Success, 0));
        assert!(!only_success.should_retry(ExecStatus::Failure, 0));

        assert!(limited_retries.should_retry(ExecStatus::Success, 0));
        assert!(limited_retries.should_retry(ExecStatus::Success, 1));
        assert!(!limited_retries.should_retry(ExecStatus::Success, 2));

        assert!(unlimited_retries.should_retry(ExecStatus::Success, 0));
        assert!(unlimited_retries.should_retry(ExecStatus::Success, 1000));
        assert!(unlimited_retries.should_retry(ExecStatus::Success, 1_000_000));
        assert!(unlimited_retries.should_retry(ExecStatus::Success, usize::MAX));
    }

    #[test]
    fn test_validate_name() {
        assert!(ChronService::validate_name("abc").is_ok());
        assert!(ChronService::validate_name("abc-def-ghi").is_ok());
        assert!(ChronService::validate_name("123-456-789").is_ok());
        assert!(ChronService::validate_name("-abc-def-ghi").is_err());
        assert!(ChronService::validate_name("abc-def-ghi-").is_err());
        assert!(ChronService::validate_name("abc--def-ghi").is_err());
        assert!(ChronService::validate_name("1*2$3").is_err());
    }
}
