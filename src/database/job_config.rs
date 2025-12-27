use crate::chron_service::Job;
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
        let job = job.as_ref();
        Self {
            schedule: match &job.scheduled_job {
                None => None,
                Some(scheduled_job) => Some(scheduled_job.read().await.get_schedule()),
            },
            command: job.definition.command.clone(),
            shell: job.shell.clone(),
            working_dir: job.definition.working_dir.clone(),
            log_dir: job.log_dir.clone(),
        }
    }
}
