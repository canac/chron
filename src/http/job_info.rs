use crate::chron_service::{Job, JobType, ProcessStatus};
use anyhow::Result;
use chrono::{DateTime, Local, Timelike};
use std::path::PathBuf;

pub struct JobInfo {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) shell: String,
    pub(crate) schedule: Option<String>,
    pub(crate) working_dir: Option<PathBuf>,
    pub(crate) next_run: Option<DateTime<Local>>,
    pub(crate) status: ProcessStatus,
    pub(crate) run_id: Option<u32>,
    pub(crate) log_dir: PathBuf,
}

impl JobInfo {
    // Generate the job info for a job
    pub(crate) async fn from_job(name: &str, job: &Job) -> Result<Self> {
        let (status, run_id) = job.running_process.read().await.as_ref().map_or_else(
            || (ProcessStatus::Terminated, None),
            |process| {
                (
                    ProcessStatus::Running { pid: process.pid },
                    Some(process.run_id),
                )
            },
        );

        let (schedule, next_run) = match job.r#type {
            JobType::Scheduled {
                ref scheduled_job, ..
            } => {
                let job_guard = scheduled_job.read().await;
                (Some(job_guard.get_schedule()), job_guard.next_run())
            }
            JobType::Startup { .. } => (None, None),
        };

        // Wait to calculate the next run until current run finishes
        let next_run = if run_id.is_none() {
            // Use the next retry attempt if it is set, falling back to the next scheduled run
            job.next_attempt
                .read()
                .await
                // Clear any fractional seconds
                .and_then(|timestamp| timestamp.with_nanosecond(0))
                .or(next_run)
                .map(DateTime::from)
        } else {
            None
        };

        Ok(Self {
            name: name.to_owned(),
            command: job.command.clone(),
            shell: job.shell.clone(),
            schedule,
            working_dir: job.working_dir.clone(),
            next_run,
            status,
            run_id,
            log_dir: job.log_dir.clone(),
        })
    }
}
