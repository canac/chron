use crate::chron_service::{Job, JobType};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobConfig {
    pub schedule: Option<String>,
    pub command: String,
    pub shell: String,
    pub working_dir: Option<PathBuf>,
    pub log_dir: PathBuf,
}

impl JobConfig {
    /// Extract a `JobConfig` from a `Job` reference
    pub async fn from_job(job: impl AsRef<Job>) -> Self {
        Self {
            schedule: match &job.as_ref().r#type {
                JobType::Startup { .. } => None,
                JobType::Scheduled { scheduled_job, .. } => {
                    Some(scheduled_job.read().await.get_schedule())
                }
            },
            command: job.as_ref().command.clone(),
            shell: job.as_ref().shell.clone(),
            working_dir: job.as_ref().working_dir.clone(),
            log_dir: job.as_ref().log_dir.clone(),
        }
    }
}
