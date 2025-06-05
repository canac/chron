use super::JobConfig;
use anyhow::{Context, anyhow, bail};
use async_sqlite::rusqlite::{Result, Row};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone, Utc};

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub enum JobStatus {
    /// The job is currently running with this process id
    Running { pid: u32 },

    /// The job is waiting for its next run at this timestamp
    Waiting { next_run: DateTime<Utc> },

    /// The job is not running and will not run again
    Completed,
}

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub struct Job {
    pub name: String,
    pub config: JobConfig,
    pub status: JobStatus,
}

pub struct RawJob {
    name: String,
    config: String,
    status: JobStatus,
}

impl RawJob {
    /// Convert an SQLite row into a Job model
    pub fn from_row(row: &Row) -> Result<Self> {
        let pid: Option<u32> = row.get("pid")?;
        let next_run: Option<NaiveDateTime> = row.get("next_run")?;
        let status = pid.map_or_else(
            || {
                next_run.map_or(JobStatus::Completed, |next_run| JobStatus::Waiting {
                    next_run: Utc.from_utc_datetime(&next_run),
                })
            },
            |pid| JobStatus::Running { pid },
        );

        Result::Ok(Self {
            name: row.get("name")?,
            config: row.get("config")?,
            status,
        })
    }
}

impl TryFrom<RawJob> for Job {
    type Error = anyhow::Error;

    /// Convert a `RawJob` model into a `Job` model
    /// This conversion will fail if the JSON job config cannot be parsed
    fn try_from(raw: RawJob) -> anyhow::Result<Self> {
        Ok(Self {
            name: raw.name,
            config: serde_json::from_str(&raw.config).context("Failed to parse job config")?,
            status: raw.status,
        })
    }
}

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub enum RunStatus {
    Running {
        pid: u32,
    },
    Completed {
        status_code: i32,
        ended_at: NaiveDateTime,
    },
    Terminated,
}

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub struct Run {
    pub id: u32,
    pub scheduled_at: NaiveDateTime,
    pub started_at: NaiveDateTime,
    pub attempt: usize,
    pub max_attempts: Option<usize>,

    state: String,
    status_code: Option<i32>,
    ended_at: Option<NaiveDateTime>,
    pid: Option<u32>,
}

impl Run {
    /// Create a Run model from a database row
    pub fn from_row(row: &Row) -> Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            scheduled_at: row.get("scheduled_at")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            status_code: row.get("status_code")?,
            attempt: row.get("attempt")?,
            max_attempts: row.get("max_attempts")?,
            state: row.get("state")?,
            pid: row.get("pid")?,
        })
    }

    /// Return the run's status
    pub fn status(&self) -> anyhow::Result<RunStatus> {
        Ok(match self.state.as_str() {
            "waiting" => bail!("Run is not running"),
            "running" => RunStatus::Running {
                pid: self
                    .pid
                    .ok_or_else(|| anyhow!("Run is running but has no pid"))?,
            },
            "completed" => match (self.status_code, self.ended_at) {
                (Some(status_code), Some(ended_at)) => RunStatus::Completed {
                    status_code,
                    ended_at,
                },
                _ => RunStatus::Terminated,
            },
            _ => bail!("Run is in an unknown state: {}", self.state),
        })
    }

    /// Return the run's start time
    pub fn started_at(&self) -> DateTime<Local> {
        Local.from_utc_datetime(&self.started_at)
    }

    /// Return the run's current elapsed execution time
    pub fn execution_time(&self) -> Option<Duration> {
        let ended_at = match self.status().ok()? {
            RunStatus::Running { .. } => Local::now(),
            RunStatus::Completed { ended_at, .. } => Local.from_utc_datetime(&ended_at),
            RunStatus::Terminated => return None,
        };
        Some(ended_at.signed_duration_since(self.started_at()))
    }
}
