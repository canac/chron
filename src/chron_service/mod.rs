mod attempt;
mod exec;
mod scheduled_job;
mod sleep;

use self::exec::exec_command;
use self::scheduled_job::ScheduledJob;
use self::sleep::sleep_until;
use crate::chronfile::{Chronfile, Config, JobDefinition, RetryConfig};
use crate::database::{HostDatabase, JobConfig};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use cron::Schedule;
use log::debug;
use std::collections::{HashMap, hash_map::Entry};
use std::mem::take;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::RwLock;
use tokio::sync::oneshot::{Receiver, Sender};
use tokio::task::JoinHandle;

pub struct Process {
    pub pid: u32,

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

pub struct Job {
    pub name: String,
    pub definition: JobDefinition,
    pub config: Arc<Config>,
    pub log_dir: PathBuf,
    pub running_process: RwLock<Option<Process>>,
    #[allow(clippy::struct_field_names)]
    pub scheduled_job: Option<RwLock<ScheduledJob>>,
    pub next_attempt: RwLock<Option<DateTime<Utc>>>,
}

impl Job {
    /// Return the time that the job is scheduled to run next
    pub async fn next_scheduled_run(&self) -> Option<DateTime<Utc>> {
        match &self.scheduled_job {
            None => None,
            Some(scheduled_job) => scheduled_job.read().await.next_run(),
        }
    }
}

pub struct Task {
    pub job: Arc<Job>,
    pub handle: JoinHandle<()>,
}

pub struct ChronService {
    log_dir: PathBuf,
    db: Arc<HostDatabase>,
    jobs: HashMap<String, Task>,
}

impl ChronService {
    /// Create a new `ChronService` instance
    pub fn new(data_dir: &Path, db: Arc<HostDatabase>) -> Self {
        Self {
            log_dir: data_dir.join("logs"),
            db,
            jobs: HashMap::new(),
        }
    }

    /// Lookup a job by name
    pub fn get_job(&self, name: &str) -> Option<&Arc<Job>> {
        self.jobs.get(name).map(|task| &task.job)
    }

    /// Start or start the chron service using the jobs defined in the provided chronfile
    pub async fn start(&mut self, chronfile: Chronfile) -> Result<()> {
        let mut existing_jobs = take(&mut self.jobs);
        let mut new_jobs = HashMap::new();

        for (name, definition) in chronfile.jobs {
            if let Entry::Occupied(entry) = existing_jobs.entry(name.clone()) {
                // Reuse the job if the definition and configuration match
                let job = &entry.get().job;
                if job.definition == definition && job.config.as_ref() == &chronfile.config {
                    debug!("{name}: reusing existing job");
                    self.jobs.insert(name, entry.remove());
                    continue;
                }
            }

            new_jobs.insert(name, definition);
        }

        // Determine any newly added jobs and lock them in the database
        if !new_jobs.is_empty() {
            let created_jobs = new_jobs.keys().cloned().collect::<Vec<_>>();
            debug!("Creating jobs: {}...", created_jobs.join(", "));
            self.db.create_jobs(created_jobs).await?;
        }

        let config = Arc::new(chronfile.config);
        for (name, definition) in new_jobs {
            self.register_job(&name, definition, Arc::clone(&config))
                .await?;
        }

        // Terminate all existing jobs that weren't reused
        self.terminate_jobs(existing_jobs).await
    }

    /// Start or start the chron service using the jobs defined in the provided chronfile
    pub async fn stop(mut self) -> Result<()> {
        let jobs = take(&mut self.jobs);
        self.terminate_jobs(jobs).await
    }

    /// Terminate a collection of jobs and wait for all of their tasks complete
    async fn terminate_jobs(&self, jobs: HashMap<String, Task>) -> Result<()> {
        // Wait for each of the tasks to complete
        let has_jobs = !jobs.is_empty();
        for (name, task) in jobs {
            debug!("{name}: waiting for job to terminate...");
            let process = task.job.running_process.write().await.take();
            if let Some(process) = process {
                process.terminate().await;
            }
            task.handle.abort();

            if let Err(err) = task.handle.await
                && err.is_panic()
            {
                debug!("{name}: failed with error: {err:?}");
            }

            self.db.uninitialize_job(name).await?;
        }
        if has_jobs {
            debug!("Finished waiting for all jobs to terminate");
        }

        Ok(())
    }

    /// Register a new job
    async fn register_job(
        &mut self,
        name: &str,
        definition: JobDefinition,
        config: Arc<Config>,
    ) -> Result<()> {
        if self.jobs.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        if let Some(schedule_str) = &definition.schedule {
            let schedule = Schedule::from_str(schedule_str).with_context(|| {
                format!("Failed to parse schedule expression {schedule_str} in job {name}")
            })?;
            self.schedule(name, definition, schedule, config).await
        } else {
            self.startup(name, definition, config).await
        }
    }

    /// Add a new job to be run on startup
    async fn startup(
        &mut self,
        name: &str,
        definition: JobDefinition,
        config: Arc<Config>,
    ) -> Result<()> {
        debug!("{name}: registering new startup job");

        let job = Arc::new(Job {
            name: name.to_owned(),
            definition,
            config,
            log_dir: self.calculate_log_dir(name),
            running_process: RwLock::new(None),
            scheduled_job: None,
            next_attempt: RwLock::new(None),
        });
        let job_copy = Arc::clone(&job);

        self.db
            .initialize_job(
                name.to_owned(),
                JobConfig::from_job(&job),
                Some(&Utc::now()),
            )
            .await?;

        let db = Arc::clone(&self.db);
        let handle = spawn(async move {
            if let Err(err) = exec_command(&db, &job, &job.definition.retry, &Utc::now()).await {
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
    async fn schedule(
        &mut self,
        name: &str,
        definition: JobDefinition,
        schedule: Schedule,
        config: Arc<Config>,
    ) -> Result<()> {
        debug!("{name}: registering new scheduled job");

        // Resume the job scheduler from the saved resume time, which is the scheduled time of the last successful (i.e.
        // not retried) run
        let resume_time = self.db.get_resume_time(name.to_owned()).await?;
        let scheduled_job = ScheduledJob::new(schedule, resume_time);
        let next_run = scheduled_job.next_run();
        let job = Arc::new(Job {
            name: name.to_owned(),
            definition,
            config,
            log_dir: self.calculate_log_dir(name),
            running_process: RwLock::new(None),
            scheduled_job: Some(RwLock::new(scheduled_job)),
            next_attempt: RwLock::new(None),
        });
        let job_copy = Arc::clone(&job);

        self.db
            .initialize_job(
                name.to_owned(),
                JobConfig::from_job(&job),
                next_run.as_ref(),
            )
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
        db: &Arc<HostDatabase>,
        name: &str,
        job: &Arc<Job>,
    ) -> Result<Option<DateTime<Utc>>> {
        let Some(scheduled_job) = job.scheduled_job.as_ref() else {
            bail!("{name}: job is not a scheduled job");
        };

        // Get the elapsed run since the last tick, if any
        let mut job_guard = scheduled_job.write().await;
        let now = Utc::now();
        let current_run = job_guard.tick(now);
        let next_run = job_guard.next_run();

        // Retry delay defaults to one sixth of the job's period
        let retry_delay = job.definition.retry.delay.unwrap_or_else(|| {
            job_guard
                .get_current_period(&now.into())
                .unwrap_or_default()
                / 6
        });

        drop(job_guard);

        if let Some(elapsed_runs) = current_run {
            let scheduled_time = elapsed_runs.oldest;
            let late =
                HumanTime::from(scheduled_time).to_text_en(Accuracy::Precise, Tense::Present);
            debug!("{name}: scheduled for {scheduled_time} ({late} late)");

            exec_command(
                db,
                job,
                &RetryConfig {
                    delay: Some(retry_delay),
                    ..job.definition.retry
                },
                &scheduled_time,
            )
            .await?;
            let resume_time = elapsed_runs.newest;
            debug!("{name}: updating resume time {resume_time}");
            db.set_resume_time(name.to_owned(), &resume_time).await?;
        }

        Ok(next_run)
    }

    // Calculate the log file directory for a job
    fn calculate_log_dir(&self, name: &str) -> PathBuf {
        self.log_dir.join(name)
    }
}
