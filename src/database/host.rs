use super::db::Database;
use super::job_config::JobConfig;
use super::models::Run;
use crate::chronfile::RetryLimit;
use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use std::path::Path;

pub struct HostDatabase {
    db: Database,
}

impl HostDatabase {
    /// Open the database as a writable host
    /// Only one host can open the database at a time, and this is enforced by `HostServer`.
    pub async fn open(chron_dir: &Path) -> Result<Self> {
        let db = Database::new(chron_dir).await?;
        db.init().await?;
        Ok(Self { db })
    }

    pub async fn insert_run(
        &self,
        name: String,
        scheduled_at: NaiveDateTime,
        attempt: usize,
        retry_limit: RetryLimit,
    ) -> Result<Run> {
        let max_attempts = match retry_limit {
            RetryLimit::Unlimited => None,
            RetryLimit::Limited(limit) => Some(limit),
        };
        self.db
            .insert_run(name, scheduled_at, attempt, max_attempts)
            .await
    }

    pub async fn set_run_pid(&self, name: String, pid: u32) -> Result<()> {
        self.db.set_run_pid(name, pid).await
    }

    pub async fn complete_run(
        &self,
        name: String,
        status_code: Option<i32>,
        next_run: Option<&DateTime<Utc>>,
    ) -> Result<()> {
        self.db.complete_run(name, status_code, next_run).await
    }

    pub async fn get_resume_time(&self, name: String) -> Result<DateTime<Utc>> {
        self.db.get_resume_time(name).await
    }

    pub async fn set_resume_time(&self, name: String, timestamp: &DateTime<Utc>) -> Result<()> {
        self.db.set_resume_time(name, timestamp).await
    }

    pub async fn create_jobs(&self, jobs: Vec<String>) -> Result<()> {
        self.db.create_jobs(jobs).await
    }

    pub async fn initialize_job(
        &self,
        name: String,
        job_config: JobConfig,
        next_run: Option<&DateTime<Utc>>,
    ) -> Result<()> {
        self.db.initialize_job(name, job_config, next_run).await
    }

    pub async fn uninitialize_job(&self, name: String) -> Result<()> {
        self.db.uninitialize_job(name).await
    }
}
