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
    pub fn from_row(row: &Row) -> rusqlite::Result<Checkpoint> {
        Ok(Checkpoint {
            timestamp: row.get("timestamp")?,
        })
    }
}

pub struct Run {
    pub id: i32,
    pub timestamp: NaiveDateTime,
    pub status_code: Option<i32>,
}

impl Run {
    pub fn from_row(row: &Row) -> rusqlite::Result<Run> {
        Ok(Run {
            id: row.get("id")?,
            timestamp: row.get("timestamp")?,
            status_code: row.get("status_code")?,
        })
    }
}
