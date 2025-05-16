use anyhow::{anyhow, bail};
use async_sqlite::rusqlite::{Result, Row};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone, Utc};

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub struct Job {
    pub name: String,
    pub command: String,
    pub status: JobStatus,
}

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub enum JobStatus {
    /// The job is currently running
    Running,

    /// The job is waiting for its next run at this timestamp
    Waiting(DateTime<Utc>),

    /// The job is not running and will not run again
    Completed,
}

impl Job {
    pub fn from_row(row: &Row) -> Result<Self> {
        let running: bool = row.get("running")?;
        let next_run: Option<NaiveDateTime> = row.get("next_run")?;
        let status = if running {
            JobStatus::Running
        } else {
            next_run.map_or(JobStatus::Completed, |next_run| {
                JobStatus::Waiting(Utc.from_utc_datetime(&next_run))
            })
        };

        Ok(Self {
            name: row.get("name")?,
            command: row.get("command")?,
            status,
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
