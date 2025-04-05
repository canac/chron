mod exec;
mod scheduled_job;
mod sleep;
mod working_dir;

use self::exec::{ExecStatus, exec_command};
use self::scheduled_job::ScheduledJob;
use self::sleep::sleep_until;
use self::working_dir::expand_working_dir;
use crate::chronfile::{self, Chronfile};
use crate::database::Database;
use anyhow::Result;
use anyhow::{Context, bail};
use chrono::{DateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use cron::Schedule;
use log::debug;
use std::collections::HashMap;
use std::mem::take;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::create_dir_all;
use tokio::spawn;
use tokio::sync::oneshot::{Receiver, Sender};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

type DatabaseMutex = Arc<Mutex<Database>>;

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
            ExecStatus::Failure => self.failures,
            ExecStatus::Success => self.successes,
        } && self.limit.is_none_or(|limit| attempt < limit))
    }
}

#[derive(Eq, PartialEq)]
pub struct StartupJobOptions {
    pub keep_alive: RetryConfig,
}

#[derive(Eq, PartialEq)]
pub struct ScheduledJobOptions {
    pub make_up_missed_run: bool,
    pub retry: RetryConfig,
}

pub struct Process {
    pub pid: u32,
    pub run_id: u32,

    /// A oneshot channel to terminate the process and receive a response when the process had finished terminating
    terminate: Option<(Sender<()>, Receiver<()>)>,
}

impl Process {
    // Terminate the process and wait for it to finish terminating
    // Return true if the process was terminated successfully
    pub async fn terminate(mut self) -> bool {
        let Some((tx_terminate, rx_terminated)) = self.terminate.take() else {
            return false;
        };
        tx_terminate.send(()).is_ok() && rx_terminated.await.is_ok()
    }
}

pub enum ProcessStatus {
    Running { pid: u32 },
    Terminated,
}

pub struct Job {
    pub name: String,
    pub command: String,
    pub shell: String,
    pub working_dir: Option<PathBuf>,
    pub log_dir: PathBuf,
    pub running_process: RwLock<Option<Process>>,
    pub r#type: JobType,
    pub next_attempt: RwLock<Option<DateTime<Utc>>>,
}

pub struct Task {
    pub job: Arc<Job>,
    pub handle: JoinHandle<Result<()>>,
}

pub struct ChronService {
    log_dir: PathBuf,
    db: DatabaseMutex,
    jobs: HashMap<String, Task>,
    default_shell: String,
    shell: Option<String>,
}

impl ChronService {
    // Create a new ChronService instance
    pub async fn new(chron_dir: &Path) -> Result<Self> {
        // Make sure that the chron directory exists
        create_dir_all(chron_dir).await?;

        let db = Database::new(chron_dir)?;
        Ok(Self {
            log_dir: chron_dir.join("logs"),
            db: Arc::new(Mutex::new(db)),
            jobs: HashMap::new(),
            default_shell: Self::get_user_shell().context("Failed to get user's default shell")?,
            shell: None,
        })
    }

    // Return the service's database connection
    pub fn get_db(&self) -> DatabaseMutex {
        Arc::clone(&self.db)
    }

    // Lookup a job by name
    pub fn get_job(&self, name: &String) -> Option<&Arc<Job>> {
        self.jobs.get(name).map(|task| &task.job)
    }

    // Return an iterator of the jobs
    pub fn get_jobs_iter(&self) -> impl Iterator<Item = (&String, &Arc<Job>)> {
        self.jobs.iter().map(|(name, task)| (name, &task.job))
    }

    // Start or start the chron service using the jobs defined in the provided chronfile
    pub async fn start(&mut self, chronfile: Chronfile) -> Result<()> {
        let same_shell = self.shell == chronfile.config.shell;
        self.shell = chronfile.config.shell;

        let mut existing_jobs = take(&mut self.jobs);

        for (name, job) in chronfile.startup_jobs {
            if job.disabled {
                continue;
            }

            if let Some((name, task)) = existing_jobs.remove_entry(&name) {
                // Reuse the job if the command, working dir, shell, and options are the same
                if task.job.command == job.command
                    && task.job.working_dir == job.working_dir
                    && same_shell
                    && matches!(&task.job.r#type, JobType::Startup { options } if **options == job.get_options())
                {
                    debug!("Reusing existing startup job {name}");
                    self.jobs.insert(name, task);
                    continue;
                }

                // Add back the job because it was not reused
                existing_jobs.insert(name, task);
            }

            debug!("Registering new startup job {name}");
            self.startup(&name, &job)?;
        }

        for (name, job) in chronfile.scheduled_jobs {
            if job.disabled {
                continue;
            }

            if let Some((name, task)) = existing_jobs.remove_entry(&name) {
                // Reuse the job if the command, working dir, shell, options, and schedule are the same
                if task.job.command == job.command
                    && task.job.working_dir == job.working_dir
                    && same_shell
                    && matches!(&task.job.r#type, JobType::Scheduled { options, scheduled_job } if **options == job.get_options() && scheduled_job.read().await.get_schedule() == job.schedule)
                {
                    debug!("Reusing existing scheduled job {name}");
                    self.jobs.insert(name, task);
                    continue;
                }

                // Add back the job because it was not reused
                existing_jobs.insert(name, task);
            }

            debug!("Registering new scheduled job {name}");
            self.schedule(&name, &job).await?;
        }

        // Terminate all existing jobs that weren't reused
        Self::terminate_jobs(existing_jobs).await;

        Ok(())
    }

    // Start or start the chron service using the jobs defined in the provided chronfile
    pub async fn stop(&mut self) {
        let jobs = take(&mut self.jobs);

        Self::terminate_jobs(jobs).await;
    }

    // Terminate a collection of jobs and wait for all of their tasks complete
    async fn terminate_jobs(jobs: HashMap<String, Task>) {
        // Wait for each of the tasks to complete
        let has_jobs = !jobs.is_empty();
        for (name, task) in jobs {
            debug!("Waiting for job {name} to terminate...");
            let process = task.job.running_process.write().await.take();
            if let Some(process) = process {
                process.terminate().await;
            }
            task.handle.abort();

            match task.handle.await {
                // The task returned an error result
                Ok(Err(err)) => debug!("Job {name} failed with error: {err:?}"),
                // The task panicked
                Err(err) if err.is_panic() => debug!("Job {name} failed with error: {err:?}"),
                _ => (),
            }
        }
        if has_jobs {
            debug!("Finished waiting for all jobs to terminate");
        }
    }

    // Add a new job to be run on startup
    fn startup(&mut self, name: &str, job: &chronfile::StartupJob) -> Result<()> {
        Self::validate_name(name)?;
        if self.jobs.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let options = Arc::new(job.get_options());
        let job = Arc::new(Job {
            name: name.to_owned(),
            command: job.command.clone(),
            shell: self.get_shell(),
            working_dir: job.working_dir.as_ref().map(expand_working_dir),
            log_dir: self.calculate_log_dir(name),
            running_process: RwLock::new(None),
            r#type: JobType::Startup {
                options: Arc::clone(&options),
            },
            next_attempt: RwLock::new(None),
        });
        let job_copy = Arc::clone(&job);

        let db = self.get_db();
        let handle = spawn(async move {
            exec_command(&db, &job, &options.keep_alive, &Utc::now()).await?;
            Ok::<(), anyhow::Error>(())
        });
        self.jobs.insert(
            job_copy.name.clone(),
            Task {
                job: job_copy,
                handle,
            },
        );

        Ok(())
    }

    // Add a new job to be run on the given schedule
    #[allow(clippy::too_many_lines)]
    async fn schedule(&mut self, name: &str, job: &chronfile::ScheduledJob) -> Result<()> {
        Self::validate_name(name)?;
        if self.jobs.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        // Resume the job scheduler from the checkpoint time, which is the
        // scheduled time of the last successful (i.e. not retried) run
        // However, jobs that don't make up missed runs resume now, not in the past
        let options = job.get_options();
        let resume_time = if options.make_up_missed_run {
            self.db.lock().await.get_checkpoint(name)?
        } else {
            None
        };

        let schedule = Schedule::from_str(&job.schedule).with_context(|| {
            format!(
                "Failed to parse schedule expression {} in job {name}",
                job.schedule
            )
        })?;
        let scheduled_job = ScheduledJob::new(schedule, resume_time.unwrap_or_else(Utc::now));
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
            if let Some(start_time) = scheduled_job.prev_run() {
                debug!("{name}: saving synthetic checkpoint {start_time}");
                self.db.lock().await.set_checkpoint(name, start_time)?;
            } else {
                debug!(
                    "{name}: cannot save synthetic checkpoint because schedule has no previous runs"
                );
            }
        }
        let job = Arc::new(Job {
            name: name.to_owned(),
            command: job.command.clone(),
            shell: self.get_shell(),
            working_dir: job.working_dir.as_ref().map(expand_working_dir),
            log_dir: self.calculate_log_dir(name),
            running_process: RwLock::new(None),
            r#type: JobType::Scheduled {
                options: Arc::new(options),
                scheduled_job: RwLock::new(Box::new(scheduled_job)),
            },
            next_attempt: RwLock::new(None),
        });
        let job_copy = Arc::clone(&job);

        let db = self.get_db();
        let name = name.to_owned();
        let handle = spawn(async move {
            loop {
                let JobType::Scheduled {
                    ref options,
                    ref scheduled_job,
                } = job.r#type
                else {
                    continue;
                };

                // Get the elapsed run since the last tick, if any
                let mut job_guard = scheduled_job.write().await;
                let now = Utc::now();
                let run = job_guard.tick(now);

                // Retry delay defaults to one sixth of the job's period
                let retry_delay = options.retry.delay.unwrap_or_else(|| {
                    job_guard
                        .get_current_period(&now.into())
                        .unwrap_or_default()
                        / 6
                });

                let next_run = job_guard.next_run();
                drop(job_guard);

                if let Some(scheduled_time) = run {
                    let late = HumanTime::from(scheduled_time)
                        .to_text_en(Accuracy::Precise, Tense::Present);
                    debug!("{name}: scheduled for {scheduled_time} ({late} late)");

                    exec_command(
                        &db,
                        &job,
                        &RetryConfig {
                            delay: Some(retry_delay),
                            ..options.retry
                        },
                        &scheduled_time,
                    )
                    .await?;
                    debug!("{name}: updating checkpoint {scheduled_time}");
                    db.lock().await.set_checkpoint(&name, scheduled_time)?;
                }

                let Some(next_run) = next_run else {
                    debug!("{name}: schedule contains no more future runs");
                    break;
                };

                // Wait until the next run before ticking again
                sleep_until(next_run).await;
            }

            Ok(())
        });

        self.jobs.insert(
            job_copy.name.clone(),
            Task {
                job: job_copy,
                handle,
            },
        );

        Ok(())
    }

    // Get the shell to execute commands with
    fn get_shell(&self) -> String {
        self.shell.as_ref().unwrap_or(&self.default_shell).clone()
    }

    // Helper to validate the job name
    fn validate_name(name: &str) -> Result<()> {
        if name.starts_with('-')
            || name.ends_with('-')
            || name.contains("--")
            || name
                .chars()
                .any(|char| !char.is_ascii_alphanumeric() && char != '-')
        {
            bail!("Invalid job name {name}")
        }

        Ok(())
    }

    // Helper to get the log file dir for a command
    fn calculate_log_dir(&self, name: &str) -> PathBuf {
        self.log_dir.join(name)
    }

    // Get the user's shell
    #[cfg(target_os = "windows")]
    fn get_user_shell() -> Result<String> {
        Ok(String::from("Invoke-Expression"))
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
