mod attempt;
mod exec;
mod scheduled_job;
mod sleep;
mod working_dir;

use self::exec::exec_command;
use self::scheduled_job::ScheduledJob;
use self::sleep::sleep_until;
use self::working_dir::expand_working_dir;
use crate::chronfile::{self, Chronfile};
use crate::database::{Database, ReserveResult};
use anyhow::Result;
use anyhow::{Context, bail};
use chrono::{DateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use cron::Schedule;
use log::debug;
use reqwest::header::HeaderValue;
use std::collections::HashMap;
use std::mem::take;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::spawn;
use tokio::sync::RwLock;
use tokio::sync::oneshot::{Receiver, Sender};
use tokio::task::JoinHandle;

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
    /// Terminate the process and wait for it to finish terminating
    // Returns `trues if the process was terminated successfully
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

impl Job {
    /// Return the time that the job is scheduled to run next
    pub async fn next_scheduled_run(&self) -> Option<DateTime<Utc>> {
        match self.r#type {
            JobType::Startup { .. } => None,
            JobType::Scheduled {
                ref scheduled_job, ..
            } => scheduled_job.read().await.next_run(),
        }
    }
}

pub struct Task {
    pub job: Arc<Job>,
    pub handle: JoinHandle<()>,
}

pub struct ChronService {
    log_dir: PathBuf,
    db: Arc<Database>,
    jobs: HashMap<String, Task>,
    default_shell: String,
    shell: Option<String>,
}

impl ChronService {
    /// Create a new `ChronService` instance
    pub fn new(data_dir: &Path, db: Arc<Database>) -> Result<Self> {
        Ok(Self {
            log_dir: data_dir.join("logs"),
            db,
            jobs: HashMap::new(),
            default_shell: Self::get_user_shell().context("Failed to get user's default shell")?,
            shell: None,
        })
    }

    /// Lookup a job by name
    pub fn get_job(&self, name: &str) -> Option<&Arc<Job>> {
        self.jobs.get(name).map(|task| &task.job)
    }

    /// Return an iterator of the jobs
    pub fn get_jobs_iter(&self) -> impl Iterator<Item = (&String, &Arc<Job>)> {
        self.jobs.iter().map(|(name, task)| (name, &task.job))
    }

    /// Determine whether a port still belongs to any running chron server
    pub fn check_port_active(port: u16) -> bool {
        let res = reqwest::blocking::get(format!("http://localhost:{port}"));
        match res {
            Ok(res) => {
                res.headers().get("x-powered-by") == Some(&HeaderValue::from_static("chron"))
            }
            Err(err) => !err.is_connect(),
        }
    }

    /// Start or start the chron service using the jobs defined in the provided chronfile
    pub async fn start(&mut self, chronfile: Chronfile, port: u16) -> Result<()> {
        let same_shell = self.shell == chronfile.config.shell;
        self.shell = chronfile.config.shell;

        let mut existing_jobs = take(&mut self.jobs);

        let mut startup_jobs = HashMap::<String, chronfile::StartupJob>::new();
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
                    debug!("{name}: reusing existing startup job");
                    self.jobs.insert(name, task);
                    continue;
                }

                // Add back the job because it was not reused
                existing_jobs.insert(name, task);
            }

            startup_jobs.insert(name, job);
        }

        let mut scheduled_jobs = HashMap::<String, chronfile::ScheduledJob>::new();
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
                    debug!("{name}: reuse existing scheduled job");
                    self.jobs.insert(name, task);
                    continue;
                }

                // Add back the job because it was not reused
                existing_jobs.insert(name, task);
            }

            scheduled_jobs.insert(name, job);
        }

        // Determine any newly added jobs and lock them in the database
        let acquired_jobs = startup_jobs
            .keys()
            .cloned()
            .chain(scheduled_jobs.keys().cloned())
            .collect::<Vec<_>>();
        if !acquired_jobs.is_empty() {
            debug!("Acquiring jobs: {}...", acquired_jobs.join(", "));
            match self
                .db
                .acquire_jobs(acquired_jobs, port, Self::check_port_active)
                .await?
            {
                ReserveResult::Reserved => {}
                ReserveResult::Failed { conflicting_jobs } => bail!(
                    "The following jobs are in use by another chron process: {}",
                    conflicting_jobs.join(", ")
                ),
            }
        }

        let unlocked_jobs = existing_jobs
            .keys()
            .filter(|job| !startup_jobs.contains_key(*job) && !scheduled_jobs.contains_key(*job))
            .cloned()
            .collect::<Vec<_>>();

        for (name, job) in startup_jobs {
            debug!("{name}: registering new startup job");
            self.startup(&name, &job).await?;
        }

        for (name, job) in scheduled_jobs {
            debug!("{name}: registering new scheduled job");
            self.schedule(&name, &job).await?;
        }

        // Terminate all existing jobs that weren't reused
        Self::terminate_jobs(existing_jobs).await;
        if !unlocked_jobs.is_empty() {
            debug!("Releasing jobs: {}", unlocked_jobs.join(", "));
            self.db.release_jobs(unlocked_jobs).await?;
        }

        Ok(())
    }

    /// Start or start the chron service using the jobs defined in the provided chronfile
    pub async fn stop(&mut self) -> Result<()> {
        let jobs = take(&mut self.jobs);

        let unlocked_jobs = jobs.keys().cloned().collect();
        Self::terminate_jobs(jobs).await;
        self.db.release_jobs(unlocked_jobs).await?;
        Ok(())
    }

    /// Terminate a collection of jobs and wait for all of their tasks complete
    async fn terminate_jobs(jobs: HashMap<String, Task>) {
        // Wait for each of the tasks to complete
        let has_jobs = !jobs.is_empty();
        for (name, task) in jobs {
            debug!("{name}: waiting for job to terminate...");
            let process = task.job.running_process.write().await.take();
            if let Some(process) = process {
                process.terminate().await;
            }
            task.handle.abort();

            if let Err(err) = task.handle.await {
                if err.is_panic() {
                    debug!("{name}: failed with error: {err:?}");
                }
            }
        }
        if has_jobs {
            debug!("Finished waiting for all jobs to terminate");
        }
    }

    /// Add a new job to be run on startup
    async fn startup(&mut self, name: &str, job: &chronfile::StartupJob) -> Result<()> {
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

        self.db
            .initialize_job(name.to_owned(), job.command.clone(), Some(&Utc::now()))
            .await?;

        let db = Arc::clone(&self.db);
        let handle = spawn(async move {
            if let Err(err) = exec_command(&db, &job, &options.keep_alive, &Utc::now()).await {
                debug!("{}: failed with error:\n{err:?}", job.name);
            }
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

    /// Add a new job to be run on the given schedule
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
            self.db.get_checkpoint(name.to_owned()).await?
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
                self.db.set_checkpoint(name.to_owned(), &start_time).await?;
            } else {
                debug!(
                    "{name}: cannot save synthetic checkpoint because schedule has no previous runs"
                );
            }
        }
        let next_run = scheduled_job.next_run();
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

        self.db
            .initialize_job(name.to_owned(), job.command.clone(), next_run.as_ref())
            .await?;

        let db = Arc::clone(&self.db);
        let name = name.to_owned();
        let handle = spawn(async move {
            loop {
                match Self::exec_scheduled_job(&db, &name, &job).await {
                    Ok(Some(next_run)) => {
                        // Wait until the next run before ticking again
                        sleep_until(next_run).await;
                    }
                    Ok(None) => {
                        debug!("{name}: schedule contains no more future runs");
                        break;
                    }
                    Err(err) => {
                        debug!("{name}: failed with error:\n{err:?}");
                        break;
                    }
                }
            }
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

    /// Execute a scheduled job a single time
    /// Returns the next time that the job is scheduled to run, if any
    async fn exec_scheduled_job(
        db: &Arc<Database>,
        name: &str,
        job: &Arc<Job>,
    ) -> Result<Option<DateTime<Utc>>> {
        let JobType::Scheduled {
            ref options,
            ref scheduled_job,
        } = job.r#type
        else {
            bail!("{name}: job is not a scheduled job");
        };

        // Get the elapsed run since the last tick, if any
        let mut job_guard = scheduled_job.write().await;
        let now = Utc::now();
        let run = job_guard.tick(now);
        let next_run = job_guard.next_run();

        // Retry delay defaults to one sixth of the job's period
        let retry_delay = options.retry.delay.unwrap_or_else(|| {
            job_guard
                .get_current_period(&now.into())
                .unwrap_or_default()
                / 6
        });

        drop(job_guard);

        if let Some(scheduled_time) = run {
            let late =
                HumanTime::from(scheduled_time).to_text_en(Accuracy::Precise, Tense::Present);
            debug!("{name}: scheduled for {scheduled_time} ({late} late)");

            exec_command(
                db,
                job,
                &RetryConfig {
                    delay: Some(retry_delay),
                    ..options.retry
                },
                &scheduled_time,
            )
            .await?;
            debug!("{name}: updating checkpoint {scheduled_time}");
            db.set_checkpoint(name.to_owned(), &scheduled_time).await?;
        }

        Ok(next_run)
    }

    /// Get the shell to execute commands with
    fn get_shell(&self) -> String {
        self.shell.as_ref().unwrap_or(&self.default_shell).clone()
    }

    /// Validate a job name
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

    // Calculate the log file directory for a job
    fn calculate_log_dir(&self, name: &str) -> PathBuf {
        self.log_dir.join(name)
    }

    /// Get the user's shell
    #[cfg(target_os = "windows")]
    fn get_user_shell() -> Result<String> {
        Ok(String::from("Invoke-Expression"))
    }

    /// Get the user's shell
    #[cfg(not(target_os = "windows"))]
    fn get_user_shell() -> Result<String> {
        std::env::var("SHELL").context("Couldn't get $SHELL environment variable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
