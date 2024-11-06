use chrono::NaiveDateTime;
use rusqlite::Row;

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
    pub fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            timestamp: row.get("timestamp")?,
        })
    }
}

pub struct Run {
    pub id: i32,
    pub scheduled_at: NaiveDateTime,
    pub started_at: NaiveDateTime,
    pub ended_at: Option<NaiveDateTime>,
    pub status_code: Option<i32>,
    pub attempt: usize,
    pub max_attempts: Option<usize>,
}

impl Run {
    pub fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            scheduled_at: row.get("scheduled_at")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            status_code: row.get("status_code")?,
            attempt: row.get("attempt")?,
            max_attempts: row.get("max_attempts")?,
        })
    }
}
