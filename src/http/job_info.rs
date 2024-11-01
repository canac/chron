use super::http_error::HttpError;
use crate::chron_service::{Job, JobType, ProcessStatus};
use actix_web::{http::StatusCode, Result};
use chrono::{DateTime, Local};
use std::path::PathBuf;

pub struct JobInfo {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) schedule: Option<String>,
    pub(crate) next_run: Option<DateTime<Local>>,
    pub(crate) status: ProcessStatus,
    pub(crate) run_id: Option<i32>,
    pub(crate) log_path: PathBuf,
}

impl JobInfo {
    // Generate the job info for a job
    pub(crate) fn from_job(name: &str, job: &Job) -> Result<Self> {
        let mut process_guard = job
            .running_process
            .write()
            .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;
        let (status, run_id) = match process_guard.as_mut() {
            Some(process) => (process.get_status()?, Some(process.run_id)),
            None => (ProcessStatus::Terminated, None),
        };
        drop(process_guard);

        let (schedule, next_run) = match job.r#type {
            JobType::Scheduled {
                ref scheduled_job, ..
            } => {
                let job_guard = scheduled_job
                    .read()
                    .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;
                (Some(job_guard.get_schedule()), job_guard.next_run())
            }
            JobType::Startup { .. } => (None, None),
        };
        Ok(Self {
            name: name.to_string(),
            command: job.command.clone(),
            schedule,
            next_run: next_run.map(DateTime::from),
            status,
            run_id,
            log_path: job.log_path.clone(),
        })
    }
}
