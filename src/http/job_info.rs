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
        // If the job is not running, the pid should be None
        let pid = match process_guard.as_mut() {
            Some(process) => {
                if process.try_wait()?.is_some() {
                    // If the process is still set but it has terminated because wait returned a status
                    None
                } else {
                    Some(process.id())
                }
            }
            None => None,
        };
        drop(process_guard);

        let (schedule, next_run) = match job.r#type {
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
