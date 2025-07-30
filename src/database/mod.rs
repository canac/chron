mod db;
mod job_config;
mod models;

use self::db::Database;
pub use self::job_config::JobConfig;
pub use self::models::{Job, JobStatus, Run, RunStatus};
use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDateTime, Utc};
use std::path::Path;

pub struct ClientDatabase {
    db: Database,
    port: u16,
}

impl ClientDatabase {
    /// Open the database as a read-only client
    /// The database will be able to read job information from an open `WritableDatabase`.
    pub async fn open(chron_dir: &Path) -> Result<Self> {
        let db = Database::new(chron_dir).await?;

        // Ensure that a host is connected to the database
        let port = db
            .get_port()
            .await?
            .ok_or_else(|| anyhow!("chron is not running"))?;

        Ok(Self { db, port })
    }

    /// Return the port of the host that opened the database
    pub fn get_port(&self) -> u16 {
        self.port
    }

    pub async fn get_last_runs(&self, name: String, count: u64) -> Result<Vec<Run>> {
        self.db.get_last_runs(name, count).await
    }

    pub async fn get_active_jobs(&self) -> Result<Vec<Job>> {
        self.db.get_active_jobs().await
    }

    pub async fn get_active_job(&self, job: String) -> Result<Option<Job>> {
        self.db.get_active_job(job).await
    }
}

pub struct HostDatabase {
    db: Database,
}

impl HostDatabase {
    /// Open the database as a writable host
    /// The database will be able to write job information that other client databases can read. Only one host can open
    /// the database at a time.
    pub async fn open(chron_dir: &Path, port: u16) -> Result<Self> {
        let db = Database::new(chron_dir).await?;
        db.init().await?;
        db.set_port(port).await?;
        Ok(Self { db })
    }

    /// Close the database, releasing it to be opened by a different host
    pub async fn close(&self) -> Result<()> {
        self.db.remove_port().await
    }

    pub async fn insert_run(
        &self,
        name: String,
        scheduled_at: NaiveDateTime,
        attempt: usize,
        max_attempts: Option<usize>,
    ) -> Result<Run> {
        self.db
            .insert_run(name, scheduled_at, attempt, max_attempts)
            .await
    }

    pub async fn set_run_pid(&self, job: String, pid: u32) -> Result<()> {
        self.db.set_run_pid(job, pid).await
    }

    pub async fn complete_run(
        &self,
        job: String,
        status_code: Option<i32>,
        next_run: Option<&DateTime<Utc>>,
    ) -> Result<()> {
        self.db.complete_run(job, status_code, next_run).await
    }

    pub async fn get_resume_time(&self, job: String) -> Result<DateTime<Utc>> {
        self.db.get_resume_time(job).await
    }

    pub async fn set_resume_time(&self, job: String, timestamp: &DateTime<Utc>) -> Result<()> {
        self.db.set_resume_time(job, timestamp).await
    }

    pub async fn create_jobs(&self, jobs: Vec<String>) -> Result<()> {
        self.db.create_jobs(jobs).await
    }

    pub async fn initialize_job(
        &self,
        name: String,
        job_config: JobConfig,
        next_run: Option<&DateTime<Utc>>,
        preserve_resume_time: bool,
    ) -> Result<()> {
        self.db
            .initialize_job(name, job_config, next_run, preserve_resume_time)
            .await
    }
}
