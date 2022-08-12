use chrono::NaiveDateTime;

/*
 * The checkpoint table records the last time that a job completed. This
 * information is used to correctly calculate missed job runs between runs
 * of chron itself. The timestamp column is the time that the job was
 * originally scheduled for, not the time that it actually ran. The timestamp
 * is only updated after completed runs, which is defined as runs don't need to
 * be retried according to the retry config.
 */
#[derive(Queryable)]
#[diesel(table_name = checkpoint)]
pub struct Checkpoint {
    pub id: i32,
    pub job: String,
    pub timestamp: NaiveDateTime,
}

#[derive(Queryable)]
#[diesel(table_name = run)]
pub struct Run {
    pub id: i32,
    pub name: String,
    pub timestamp: NaiveDateTime,
    pub status_code: Option<i32>,
}
