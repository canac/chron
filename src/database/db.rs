use super::job_config::JobConfig;
use super::models::{Job, Run};
use crate::database::models::RawJob;
use anyhow::{Context, Result, bail};
use async_sqlite::rusqlite::ToSql;
use async_sqlite::rusqlite::{self};
use async_sqlite::{Client, ClientBuilder, JournalMode};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use std::convert::TryInto;
use std::path::Path;

pub struct Database {
    client: Client,
}

impl Database {
    /// Create a new Database instance
    pub async fn new(chron_dir: &Path) -> Result<Self> {
        let db_path = chron_dir.join("chron.db");
        let client = ClientBuilder::new()
            .path(db_path.clone())
            .journal_mode(JournalMode::Wal)
            .open()
            .await
            .with_context(|| format!("Failed to open SQLite database {}", db_path.display()))?;
        Ok(Self { client })
    }

    /// Create the database tables
    pub async fn init(&self) -> Result<()> {
        self.client
            .conn(|conn| {
                conn.execute_batch(
                    "
CREATE TABLE IF NOT EXISTS job (
  name VARCHAR PRIMARY KEY NOT NULL,
  initialized BOOLEAN NOT NULL DEFAULT FALSE,
  config JSON,
  resume_at DATETIME,
  next_run DATETIME
);

CREATE TABLE IF NOT EXISTS run (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_name VARCHAR NOT NULL REFERENCES job(name),
  state VARCHAR NOT NULL DEFAULT 'starting' CHECK(state IN ('starting', 'running', 'completed')),
  scheduled_at DATETIME NOT NULL,
  started_at DATETIME NOT NULL DEFAULT (STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW')),
  ended_at DATETIME,
  pid INTEGER,
  status_code INTEGER,
  attempt INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER
);

CREATE TABLE IF NOT EXISTS host_info (
  key VARCHAR PRIMARY KEY NOT NULL,
  value NOT NULL
);",
                )
            })
            .await
            .context("Failed to create SQLite tables")?;

        self.client
            .conn(|conn| {
                // Add a busy timeout so that when multiple processes try to write to
                // the database, they wait for each other to finish instead of erroring
                conn.execute_batch("PRAGMA busy_timeout = 1000")
            })
            .await
            .context("Failed to set busy timeout")?;

        self.client
            .conn(|conn| {
                // Data integrity: existing jobs start out as uninitialized and existing runs start out as completed on
                // a new host
                // This ensures that if the process crashed while jobs were running, they are considered terminated when
                // the database is opened again
                conn.execute_batch(
                    "
UPDATE job
SET initialized = FALSE;

UPDATE run
SET state = 'completed';",
                )
            })
            .await
            .context("Failed to reset jobs and runs in the database")?;

        Ok(())
    }

    /// Return the port of the host currently connected to the database
    pub async fn get_port(&self) -> Result<Option<u16>> {
        self.client
            .conn(|conn| {
                let mut statement = conn.prepare(
                    "
SELECT value
FROM host_info
WHERE key = 'port'",
                )?;
                let mut rows = statement.query_map([], |row| row.get::<_, u16>("value"))?;
                rows.next().transpose()
            })
            .await
            .context("Failed to get port from the database")
    }

    /// Set the port in the host info table, marking the database as in-use
    pub async fn set_port(&self, port: u16) -> Result<()> {
        let result = self
            .client
            .conn(move |conn| {
                conn.execute(
                    "
INSERT INTO host_info (key, value)
VALUES ('port', ?1)",
                    (port,),
                )
            })
            .await;

        if let Err(ref err) = result
            && let async_sqlite::Error::Rusqlite(err) = err
            && let Some(err) = err.sqlite_error()
            && err.code == rusqlite::ErrorCode::ConstraintViolation
        {
            bail!("chron is already running")
        }

        result.context("Failed to set port in the database")?;
        Ok(())
    }

    /// Remove the port from the host info table, marking the database as no longer in-use
    pub async fn remove_port(&self) -> Result<()> {
        self.client
            .conn(move |conn| {
                conn.execute(
                    "
DELETE FROM host_info
WHERE key = 'port'",
                    (),
                )
            })
            .await
            .context("Failed to remove port from the database")?;
        Ok(())
    }

    /// Record a new run in the database and return the id of the new run
    pub async fn insert_run(
        &self,
        name: String,
        scheduled_at: NaiveDateTime,
        attempt: usize,
        max_attempts: Option<usize>,
    ) -> Result<Run> {
        self.client
            .conn(move |conn| {
                conn.query_row(
                    "
INSERT INTO run (job_name, scheduled_at, attempt, max_attempts)
VALUES (?1, ?2, ?3, ?4)
RETURNING *",
                    (name.clone(), scheduled_at, attempt, max_attempts),
                    Run::from_row,
                )
            })
            .await
            .context("Failed to save run to the database")
    }

    /// Set a job's current run's process id
    pub async fn set_run_pid(&self, job: String, pid: u32) -> Result<()> {
        self.client
            .conn(move |conn| {
                conn.prepare(
                    "
UPDATE run
SET pid = ?1, state = 'running'
WHERE id = (
    SELECT MAX(id) as id
    FROM run
    WHERE job_name = ?2 AND state = 'starting'
)",
                )?
                .execute((pid, job))
            })
            .await
            .context("Failed to set run process id in the database")?;

        Ok(())
    }

    /// Mark a job's current run as completed with a given status code and set its next run time
    pub async fn complete_run(
        &self,
        job: String,
        status_code: Option<i32>,
        next_run: Option<&DateTime<Utc>>,
    ) -> Result<()> {
        let next_run = next_run.map(chrono::DateTime::naive_utc);
        self.client
            .conn(move |conn| {
                conn.execute_batch("BEGIN TRANSACTION")?;

                conn.execute(
                    "
UPDATE job
SET next_run = ?1
WHERE name = ?2",
                    (next_run, job.clone()),
                )?;

                conn.execute(
                    "
UPDATE run
SET state = 'completed', ended_at = STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'), status_code = ?1
WHERE id = (
    SELECT MAX(id)
    FROM run
    WHERE job_name = ?2 AND state = 'running'
)",
                    (status_code, job),
                )?;

                conn.execute_batch("COMMIT")
            })
            .await
            .context("Failed to update run status in the database")?;
        Ok(())
    }

    /// Read the last runs of a job
    /// Data integrity: forcefully terminated runs will still appear as running until the job is created again
    pub async fn get_last_runs(&self, name: String, count: u64) -> Result<Vec<Run>> {
        self.client
            .conn(move |conn| {
                let mut statement = conn.prepare(
                    "
SELECT *
FROM run
WHERE job_name = ?1 AND state != 'starting'
ORDER BY run.id DESC
LIMIT ?2",
                )?;
                statement
                    .query_map((name, count), Run::from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await
            .context("Failed to load last runs from the database")
    }

    /// Read the resume time of a job, setting it to the current time if it isn't set
    /// The resume time records the last time that a scheduled job successfully completed. It is used to calculated
    /// missed job runs between runs of chron itself. The timestamp It is the time that the run was originally scheduled
    /// for, not the time that it actually ran. The resume time is only updated after completed runs, not runs that were
    /// configured to be retried.
    pub async fn get_resume_time(&self, job: String) -> Result<DateTime<Utc>> {
        let resume_at = self
            .client
            .conn(|conn| {
                // If the job doesn't have a saved resume time yet, set it to the current time. This is to prevent jobs
                // with a long period from never running if chron isn't running when they are scheduled to run. For
                // example, assume the following scenario:
                // * A job is scheduled to run on Sundays at midnight
                // * chron is stopped on Saturday before the job runs
                // * chron is restarted on Monday after the job was scheduled
                // When it restarts on Monday, because there isn't a resume time, it will schedule the job for the next
                // Sunday. Eagerly, saving a resume time prevents this problem.
                let mut statement = conn.prepare(
                    "
UPDATE job
SET resume_at = COALESCE(resume_at, STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'))
WHERE name = ?1
RETURNING resume_at",
                )?;
                statement.query_row((job,), |row| row.get::<_, NaiveDateTime>("resume_at"))
            })
            .await
            .context("Failed to load resume time from the database")?;
        Ok(Utc.from_utc_datetime(&resume_at))
    }

    /// Write the resume time of a job
    pub async fn set_resume_time(&self, job: String, timestamp: &DateTime<Utc>) -> Result<()> {
        let timestamp = timestamp.naive_utc();
        self.client
            .conn(move |conn| {
                let mut statement = conn.prepare(
                    "
UPDATE job
SET resume_at = ?1
WHERE name = ?2",
                )?;
                statement.execute((timestamp, job))
            })
            .await
            .context("Failed to save resume time to the database")?;
        Ok(())
    }

    /// Create new, uninitialized jobs
    /// The caller should call `initialize_job` shortly thereafter
    pub async fn create_jobs(&self, jobs: Vec<String>) -> Result<()> {
        self.client
            .conn(move |conn| {
                // Data integrity: newly-created jobs are uninitialized and have no current run
                let mut statement = conn.prepare(
                    "
INSERT INTO job (name)
VALUES (?1)
ON CONFLICT (name)
    DO UPDATE
    SET initialized = FALSE",
                )?;
                for job in jobs {
                    statement.execute((job,))?;
                }
                Ok(())
            })
            .await
            .context("Failed to create jobs in the database")?;

        Ok(())
    }

    /// Record information about a newly-created job
    /// The job's resume time is set to NULL unless `preserve_resume_time` is true.
    pub async fn initialize_job(
        &self,
        name: String,
        job_config: JobConfig,
        next_run: Option<&DateTime<Utc>>,
        preserve_resume_time: bool,
    ) -> Result<()> {
        let serialized_config = serde_json::to_string(&job_config)?;
        let next_run = next_run.map(chrono::DateTime::naive_utc);
        self.client
            .conn(move |conn| {
                conn.execute(
                    "
UPDATE job
SET config = ?1, next_run = ?2, initialized = TRUE, resume_at = CASE WHEN ?3 THEN resume_at ELSE NULL END
WHERE name = ?4",
                    (serialized_config, next_run, preserve_resume_time, name),
                )
            })
            .await
            .context("Failed to initialize jobs in the database")?;

        Ok(())
    }

    /// Return all running, initialized jobs
    pub async fn get_active_jobs(&self) -> Result<Vec<Job>> {
        self.internal_get_active_jobs(None).await
    }

    /// Return a running, initialized job by its name
    pub async fn get_active_job(&self, job: String) -> Result<Option<Job>> {
        let jobs = self.internal_get_active_jobs(Some(job)).await?;
        Ok(jobs.into_iter().next())
    }

    /// Internal implementation of `get_active_jobs` and `get_active_job`
    /// Supports optionally filtering down to a single active job
    async fn internal_get_active_jobs(&self, job: Option<String>) -> Result<Vec<Job>> {
        self.client
            .conn(move |conn| {
                conn.execute_batch("BEGIN TRANSACTION")?;

                let mut params: Vec<(&str, &dyn ToSql)> = vec![];
                let name_field = if job.is_some() {
                    params.push((":name", &job));
                    ":name"
                } else {
                    // Make the WHERE constraint a no-op
                    "name"
                };

                // Data integrity: released and uninitialized jobs are ignored
                let mut statement = conn.prepare(
                    format!(
                        "
WITH current_runs AS (
    SELECT job_name, pid
    FROM run r
    WHERE state = 'running'
    AND id = (
        SELECT MAX(id)
        FROM run
        WHERE job_name = r.job_name
    )
)
SELECT name, config, next_run, current_runs.pid AS pid
FROM job
LEFT JOIN current_runs ON current_runs.job_name = job.name
WHERE name = {name_field} AND initialized = TRUE
ORDER BY name",
                    )
                    .as_str(),
                )?;
                let jobs = statement
                    .query_map(params.as_slice(), RawJob::from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;

                conn.execute_batch("COMMIT")?;

                Ok(jobs)
            })
            .await
            .context("Failed to get active jobs in the database")?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::database::{JobStatus, RunStatus};
    use assert_matches::assert_matches;
    use chrono::Days;
    use tokio::test;

    use super::*;

    async fn open_db() -> Database {
        let client = ClientBuilder::new().open().await.unwrap();
        let db = Database { client };
        db.init().await.unwrap();
        db
    }

    async fn initialize_job(db: &Database, name: String) {
        db.initialize_job(name, JobConfig::default(), None, false)
            .await
            .unwrap();
    }

    async fn insert_run(db: &Database, name: String) -> u32 {
        let id = db
            .insert_run(name.clone(), Utc::now().naive_utc(), 0, None)
            .await
            .unwrap()
            .id;
        db.set_run_pid(name, 0).await.unwrap();
        id
    }

    #[test]
    async fn test_set_port() {
        let db = open_db().await;

        db.set_port(1000).await.unwrap();
        assert_eq!(
            db.set_port(1000).await.unwrap_err().to_string(),
            "chron is already running"
        );
    }

    #[test]
    async fn test_remove_port() {
        let db = open_db().await;

        db.set_port(1000).await.unwrap();
        assert_eq!(db.get_port().await.unwrap(), Some(1000));

        db.remove_port().await.unwrap();
        assert_eq!(db.get_port().await.unwrap(), None);

        db.set_port(1000).await.unwrap();
        assert_eq!(db.get_port().await.unwrap(), Some(1000));
    }

    #[test]
    async fn test_insert_run() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.create_jobs(vec![name.clone()]).await.unwrap();
        insert_run(&db, name.clone()).await;

        let runs = db.get_last_runs(name, 1).await.unwrap();
        let run = runs.first().unwrap();
        assert_eq!(run.status().unwrap(), RunStatus::Running { pid: 0 });
    }

    #[test]
    async fn test_insert_run_updates_job() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.create_jobs(vec![name.clone()]).await.unwrap();
        let next_run = Utc::now();
        db.initialize_job(name.clone(), JobConfig::default(), Some(&next_run), false)
            .await
            .unwrap();
        assert_eq!(
            db.get_active_jobs().await.unwrap().first().unwrap().status,
            JobStatus::Waiting { next_run },
        );

        insert_run(&db, name.clone()).await;

        assert_eq!(
            db.get_active_jobs().await.unwrap().first().unwrap().status,
            JobStatus::Running { pid: 0 },
        );
    }

    #[test]
    async fn test_complete_run() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.create_jobs(vec![name.clone()]).await.unwrap();
        db.initialize_job(name.clone(), JobConfig::default(), None, false)
            .await
            .unwrap();
        insert_run(&db, name.clone()).await;
        db.complete_run(name.clone(), Some(0), None).await.unwrap();

        let runs = db.get_last_runs(name, 1).await.unwrap();
        let run = runs.first().unwrap();
        assert_matches!(
            run.status().unwrap(),
            RunStatus::Completed { status_code: 0, .. }
        );

        assert_eq!(
            db.get_active_jobs().await.unwrap().first().unwrap().status,
            JobStatus::Completed,
        );
    }

    #[test]
    async fn test_get_last_runs() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.create_jobs(vec![name.clone()]).await.unwrap();
        db.insert_run(name.clone(), Utc::now().naive_utc(), 2, Some(3))
            .await
            .unwrap();

        // The run is in the starting state and is ignored
        assert_eq!(db.get_last_runs(name.clone(), 1).await.unwrap().len(), 0);

        // Now the run is running
        db.set_run_pid(name.clone(), 0).await.unwrap();

        let runs = db.get_last_runs(name.clone(), 1).await.unwrap();
        assert_eq!(runs.len(), 1);
        let run = runs.first().unwrap();
        assert_eq!(run.id, 1);
        assert_eq!(run.attempt, 2);
        assert_eq!(run.max_attempts, Some(3));
        assert_eq!(run.status().unwrap(), RunStatus::Running { pid: 0 });

        db.complete_run(name.clone(), Some(0), None).await.unwrap();
        db.insert_run(name.clone(), Utc::now().naive_utc(), 1, Some(3))
            .await
            .unwrap();
        db.set_run_pid(name.clone(), 1).await.unwrap();
        let runs = db.get_last_runs(name.clone(), 2).await.unwrap();
        assert_eq!(runs.len(), 2);
        // The runs are sorted by started_at descending
        assert_eq!(runs[0].id, 2);
        assert_eq!(runs[1].id, 1);
        // The newly-inserted run is the current run and the old insert run is not the current
        assert_eq!(runs[0].status().unwrap(), RunStatus::Running { pid: 1 });
        assert_matches!(
            runs[1].status().unwrap(),
            RunStatus::Completed { status_code: 0, .. }
        );

        db.complete_run(name.clone(), None, None).await.unwrap();
        let runs = db.get_last_runs(name, 1).await.unwrap();
        let run = runs.first().unwrap();
        assert_eq!(run.status().unwrap(), RunStatus::Terminated);
    }

    #[test]
    async fn test_get_last_runs_incomplete_run() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.create_jobs(vec![name.clone()]).await.unwrap();
        insert_run(&db, name.clone()).await;
        // Simulate a new host connecting to the database without completing the run
        db.init().await.unwrap();

        let runs = db.get_last_runs(name.clone(), 1).await.unwrap();
        assert_eq!(runs[0].status().unwrap(), RunStatus::Terminated);
    }

    #[test]
    async fn test_get_resume_time_sets_to_now() {
        let db = open_db().await;
        db.create_jobs(vec!["job1".to_owned()]).await.unwrap();
        db.initialize_job("job1".to_owned(), JobConfig::default(), None, false)
            .await
            .unwrap();

        let resume_time = db.get_resume_time("job1".to_owned()).await.unwrap();
        assert_eq!(
            resume_time,
            db.get_resume_time("job1".to_owned()).await.unwrap(),
            "Resume time does not change"
        );
        assert_eq!((Utc::now() - resume_time).num_seconds(), 0);
    }

    #[test]
    async fn test_create_jobs_uninitializes() {
        let db = open_db().await;

        // Seed with an old run and recreate
        db.create_jobs(vec!["job1".to_owned()]).await.unwrap();
        initialize_job(&db, "job1".to_owned()).await;
        insert_run(&db, "job1".to_owned()).await;
        db.create_jobs(vec!["job1".to_owned()]).await.unwrap();

        // The job should not be active be because it is uninitialized
        assert_eq!(db.get_active_jobs().await.unwrap(), vec![]);
    }

    #[test]
    async fn test_initialize_job() {
        let db = open_db().await;
        db.create_jobs(vec!["job1".to_owned()]).await.unwrap();
        let make_config = || JobConfig {
            command: "echo 'Hello, World!'".to_owned(),
            ..JobConfig::default()
        };

        db.initialize_job("job1".to_owned(), make_config(), None, false)
            .await
            .unwrap();

        assert_eq!(
            db.get_active_jobs().await.unwrap(),
            vec![Job {
                name: "job1".to_owned(),
                config: make_config(),
                status: JobStatus::Completed,
            }]
        );
    }

    #[test]
    async fn test_initialize_job_preserve_resume_time() {
        let db = open_db().await;
        db.create_jobs(vec!["job1".to_owned()]).await.unwrap();
        let resume_time = Utc::now().checked_sub_days(Days::new(1)).unwrap();
        db.set_resume_time("job1".to_owned(), &resume_time)
            .await
            .unwrap();
        db.initialize_job("job1".to_owned(), JobConfig::default(), None, true)
            .await
            .unwrap();

        assert_eq!(
            db.get_resume_time("job1".to_owned()).await.unwrap(),
            resume_time,
        );
    }

    #[test]
    async fn test_initialize_job_clear_resume_time() {
        let db = open_db().await;
        db.create_jobs(vec!["job1".to_owned()]).await.unwrap();
        let resume_time = Utc::now();
        db.set_resume_time("job1".to_owned(), &resume_time)
            .await
            .unwrap();
        db.initialize_job("job1".to_owned(), JobConfig::default(), None, false)
            .await
            .unwrap();

        assert_ne!(
            db.get_resume_time("job1".to_owned()).await.unwrap(),
            resume_time,
        );
    }

    #[test]
    async fn test_create_jobs_empty() {
        let db = open_db().await;
        db.create_jobs(Vec::new()).await.unwrap();
    }

    #[test]
    async fn test_get_active_jobs_uninitialized() {
        let db = open_db().await;
        db.create_jobs(vec!["job1".to_owned(), "job2".to_owned()])
            .await
            .unwrap();

        assert_eq!(db.get_active_jobs().await.unwrap(), vec![]);
    }

    #[test]
    async fn test_get_active_jobs_partially_initialized() {
        let db = open_db().await;
        db.create_jobs(vec!["job1".to_owned(), "job2".to_owned()])
            .await
            .unwrap();
        initialize_job(&db, "job1".to_owned()).await;

        assert_eq!(
            db.get_active_jobs()
                .await
                .unwrap()
                .into_iter()
                .map(|job| job.name)
                .collect::<Vec<_>>(),
            vec!["job1".to_owned()]
        );
    }

    #[test]
    async fn test_get_active_jobs_inactive() {
        let db = open_db().await;
        let jobs = vec!["job1".to_owned(), "job2".to_owned()];
        db.create_jobs(jobs.clone()).await.unwrap();
        db.create_jobs(vec!["job3".to_owned()]).await.unwrap();
        initialize_job(&db, "job1".to_owned()).await;
        initialize_job(&db, "job2".to_owned()).await;

        assert_eq!(
            db.get_active_jobs()
                .await
                .unwrap()
                .into_iter()
                .map(|job| job.name)
                .collect::<Vec<_>>(),
            jobs
        );
    }

    #[test]
    async fn test_get_active_jobs_running() {
        let db = open_db().await;

        db.create_jobs(vec!["job1".to_owned()]).await.unwrap();
        // The job should not active be because it has not been initialized yet
        assert_eq!(db.get_active_jobs().await.unwrap(), vec![]);

        initialize_job(&db, "job1".to_owned()).await;
        // The job should not be running because it does not have an active run yet
        assert_eq!(
            db.get_active_jobs().await.unwrap().first().unwrap().status,
            JobStatus::Completed,
        );

        insert_run(&db, "job1".to_owned()).await;
        assert_eq!(
            db.get_active_jobs().await.unwrap().first().unwrap().status,
            JobStatus::Running { pid: 0 },
        );
    }

    #[test]
    async fn test_get_active_job() {
        let db = open_db().await;

        db.create_jobs(vec!["job1".to_owned()]).await.unwrap();
        initialize_job(&db, "job1".to_owned()).await;
        insert_run(&db, "job1".to_owned()).await;
        assert_eq!(
            db.get_active_job("job1".to_owned())
                .await
                .unwrap()
                .unwrap()
                .status,
            JobStatus::Running { pid: 0 },
        );
    }
}
