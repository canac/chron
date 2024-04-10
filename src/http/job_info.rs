use crate::chron_service::{Job, JobType};
use actix_web::Result;
use chrono::{DateTime, Local};
use std::path::PathBuf;

pub(crate) struct JobInfo {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) schedule: Option<String>,
    pub(crate) next_run: Option<DateTime<Local>>,
    pub(crate) pid: Option<u32>,
    pub(crate) log_path: PathBuf,
}

impl JobInfo {
    // Generate the job info for a job
    pub(crate) fn from_job(name: &str, job: &Job) -> Result<JobInfo> {
        let mut process_guard = job.process.write().unwrap();
        // The job is running if the process is set and try_wait returns None
        let pid = match process_guard.as_mut() {
            Some(process) => process.try_wait()?.map(|_| process.id()),
            None => None,
        };
        drop(process_guard);

        let (schedule, next_run) = match job.job_type {
            JobType::Scheduled {
                ref scheduled_job, ..
            } => {
                let job_guard = scheduled_job.read().unwrap();
                (Some(job_guard.get_schedule()), job_guard.next_run())
            }
            JobType::Startup { .. } => (None, None),
        };
        Ok(JobInfo {
            name: name.to_string(),
            command: job.command.clone(),
            schedule,
            next_run: next_run.map(DateTime::from),
            pid,
            log_path: job.log_path.clone(),
        })
    }
}
