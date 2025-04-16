use async_sqlite::rusqlite::{Result, Row};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone};

#[derive(Debug)]
pub struct Job {
    pub name: String,
    pub port: u16,
}

impl Job {
    pub fn from_row(row: &Row) -> Result<Self> {
        Ok(Self {
            name: row.get("name")?,
            port: row.get("port")?,
        })
    }
}

/*
 * The checkpoint table records the last time that a job completed. This
 * information is used to correctly calculate missed job runs between runs
 * of chron itself. The timestamp column is the time that the job was
 * originally scheduled for, not the time that it actually ran. The timestamp
 * is only updated after completed runs, which is defined as runs don't need to
 * be retried according to the retry config.
 */
pub struct Checkpoint {
    pub timestamp: NaiveDateTime,
}

impl Checkpoint {
    /// Create a Checkpoint model from a database row
    pub fn from_row(row: &Row) -> Result<Self> {
        Ok(Self {
            timestamp: row.get("timestamp")?,
        })
    }
}

pub enum RunStatus {
    Running,
    Completed { status_code: i32 },
    Terminated,
}

pub struct Run {
    pub id: u32,
    pub scheduled_at: NaiveDateTime,
    pub started_at: NaiveDateTime,
    pub ended_at: Option<NaiveDateTime>,
    pub status_code: Option<i32>,
    pub attempt: usize,
    pub max_attempts: Option<usize>,
    pub current: bool,
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
            current: false,
        })
    }

    /// Augment the run with information about the job's current run id, making `status` and
    /// `execution_time` more accurate for currently running jobs
    pub fn with_current_run_id(mut self, current_run_id: Option<u32>) -> Self {
        if Some(self.id) == current_run_id {
            self.current = true;
        }
        self
    }

    /// Return the run's status
    pub fn status(&self) -> RunStatus {
        if self.current {
            RunStatus::Running
        } else {
            self.status_code
                .map_or(RunStatus::Terminated, |status_code| RunStatus::Completed {
                    status_code,
                })
        }
    }

    /// Return the run's start time
    pub fn started_at(&self) -> DateTime<Local> {
        Local.from_utc_datetime(&self.started_at)
    }

    /// Return the run's current elapsed execution time
    pub fn execution_time(&self) -> Option<Duration> {
        let ended_at = if self.current {
            Some(Local::now())
        } else {
            self.ended_at
                .map(|timestamp| Local.from_utc_datetime(&timestamp))
        };
        ended_at.map(|ended_at| ended_at.signed_duration_since(self.started_at()))
    }
}
