mod models;

pub use self::models::{Checkpoint, Job, JobStatus, Run, RunStatus};
use anyhow::{Context, Result};
use async_sqlite::rusqlite::Connection;
use async_sqlite::rusqlite::{self, OptionalExtension, types::Value, vtab::array::load_module};
use async_sqlite::{Client, ClientBuilder, JournalMode};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use log::info;
use std::collections::HashSet;
use std::iter::once;
use std::path::Path;
use std::rc::Rc;

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub enum ReserveResult {
    Reserved,
    Failed { conflicting_jobs: Vec<String> },
}

/// # Data Integrity
/// The data model is designed to ensure data integrity and consistency even when used concurrently by multiple chron
/// processes, even when chron processes are terminated before they can shut down gracefully.
///
/// The following invariants are maintained:
/// * Two processes cannot both acquire a job with a given name at the same time
/// * Jobs belonging to terminated chron processes are recovered and can be reacquired
///
/// The database makes the following assumptions:
/// * Each chron process is associated with exactly one port
/// * It is possible to determine whether a chron process is still running by attempting to connect
///   to its port over HTTP (the exact method used to send the HTTP request and inspect the
///   response is left to the caller)
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
            .with_context(|| format!("Failed to open SQLite database {db_path:?}"))?;

        let db = Self { client };
        db.init().await?;
        Ok(db)
    }

    /// Create the database tables
    async fn init(&self) -> Result<()> {
        self.client
            .conn(|conn| {
                conn.execute_batch(
                    "
CREATE TABLE IF NOT EXISTS job (
  name VARCHAR PRIMARY KEY NOT NULL,
  port INTEGER,
  initialized BOOLEAN NOT NULL DEFAULT FALSE,
  command VARCHAR,
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

CREATE TABLE IF NOT EXISTS checkpoint (
  id INTEGER PRIMARY KEY,
  job VARCHAR NOT NULL UNIQUE,
  timestamp DATETIME NOT NULL
);",
                )?;

                load_module(conn)?;

                Ok(())
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
                    "INSERT INTO run (job_name, scheduled_at, attempt, max_attempts)
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
                    "UPDATE run
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
                conn.execute_batch("BEGIN")?;

                conn.execute(
                    "UPDATE job
SET next_run = ?1
WHERE name = ?2",
                    (next_run, job.clone()),
                )?;

                conn.execute(
                    "UPDATE run
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
    /// Data integrity: forcefully terminated runs will still appear as running until the job is acquired again
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

    /// Read the checkpoint time of a job
    pub async fn get_checkpoint(&self, job: String) -> Result<Option<DateTime<Utc>>> {
        let checkpoint = self
            .client
            .conn(|conn| {
                let mut statement = conn.prepare(
                    "SELECT timestamp
FROM checkpoint
WHERE job = ?1",
                )?;
                statement.query_row((job,), Checkpoint::from_row).optional()
            })
            .await
            .context("Failed to save run to the database")?;
        Ok(checkpoint.map(|checkpoint| Utc.from_utc_datetime(&checkpoint.timestamp)))
    }

    /// Write the checkpoint time of a job
    pub async fn set_checkpoint(&self, job: String, timestamp: &DateTime<Utc>) -> Result<()> {
        let timestamp = timestamp.naive_utc();
        self.client
            .conn(move |conn| {
                let mut statement = conn.prepare(
                    "INSERT INTO checkpoint (job, timestamp)
VALUES (?1, ?2)
ON CONFLICT (job)
    DO UPDATE
    SET timestamp = (?2)",
                )?;
                statement.execute((job, timestamp))
            })
            .await
            .context("Failed to save checkpoint time to the database")?;
        Ok(())
    }

    /// Get the port currently associated with a job, if any
    pub async fn get_job_port(&self, job: String) -> Result<Option<u16>> {
        let port = self
            .client
            .conn(move |conn| {
                let mut statement = conn.prepare(
                    "SELECT port
FROM job
WHERE name = ?1 AND port IS NOT NULL",
                )?;
                statement
                    .query_row((job,), |row| row.get("port"))
                    .optional()
            })
            .await?;
        Ok(port)
    }

    /// Attempt to associate the given jobs with the current process, identified by its port
    /// Reacquiring jobs already associated with this port is a noop
    /// This will fail if another process has already acquired the jobs
    /// Acquired, running jobs are guaranteed to have accurate current run information. Released jobs make no such
    /// guarantees, so callers should verify that the job is actually running before trusting information from the
    /// database about the job's current run. Acquired jobs also begin as uninitialized, so the caller should call
    /// `initialize_job` shortly thereafter.
    pub async fn acquire_jobs(
        &self,
        jobs: Vec<String>,
        port: u16,
        check_port_active: impl Fn(u16) -> bool + Send + 'static,
    ) -> Result<ReserveResult> {
        struct Job {
            name: String,
            port: u16,
        }

        impl Job {
            fn from_row(row: &rusqlite::Row) -> async_sqlite::rusqlite::Result<Self> {
                Ok(Self {
                    name: row.get("name")?,
                    port: row.get("port")?,
                })
            }
        }

        // Data integrity: use an exclusive transaction to ensure that the job and port associations are not modified
        // while we are determining which jobs are acquired and/or recoverable
        let result = self
            .client
            .conn(move |conn| {
                conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION")?;

                let mut statement = conn.prepare(
                    "SELECT name, port
FROM job
WHERE name IN rarray(?1) AND port IS NOT NULL AND port != ?2",
                )?;
                let conflicting_jobs = statement
                    .query_map((Self::make_rarray(jobs.clone()), port), Job::from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;

                // Given the jobs that are already in use, determine which ports they are associated and check whether
                // those ports still belong to a running chron server. Then use that information to determine which
                // conflicting jobs can be automatically released and which are still in use.
                let ports = conflicting_jobs
                    .iter()
                    .map(|job| job.port)
                    .collect::<HashSet<_>>();
                let recovered_ports = ports
                    .into_iter()
                    .filter(|job_port| !check_port_active(*job_port))
                    .collect::<HashSet<_>>();
                let (released_jobs, conflicting_jobs) = conflicting_jobs
                    .into_iter()
                    .partition::<Vec<_>, _>(|job| recovered_ports.contains(&job.port));
                if !released_jobs.is_empty() {
                    info!(
                        "Recovered jobs: {}",
                        released_jobs
                            .iter()
                            .map(|job| job.name.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }

                // Whether or not conflicts remain, release any recoverable jobs
                Self::release_by_ports(conn, recovered_ports)?;

                if !conflicting_jobs.is_empty() {
                    conn.execute_batch("COMMIT")?;

                    let conflicting_jobs =
                        conflicting_jobs.into_iter().map(|job| job.name).collect();
                    return Ok(ReserveResult::Failed { conflicting_jobs });
                }

                // If this job already exists, it must be already acquired by this port and is safe to update
                // Data integrity: newly-acquired jobs are uninitialized and have no current run
                let mut statement = conn.prepare(
                    "INSERT INTO job (name, port)
VALUES (?1, ?2)
ON CONFLICT (name)
    DO UPDATE
    SET port = ?2, initialized = FALSE",
                )?;
                for job in jobs {
                    statement.execute((job, port))?;
                }
                conn.execute_batch("COMMIT")?;
                Ok(ReserveResult::Reserved)
            })
            .await
            .context("Failed to acquire jobs in the database")?;

        Ok(result)
    }

    /// Release previously acquired jobs
    pub async fn release_jobs(&self, jobs: Vec<String>) -> Result<()> {
        self.client
            .conn(move |conn| Self::release_by_names(conn, jobs))
            .await
            .context("Failed to release jobs by name in the database")?;

        Ok(())
    }

    /// Release all previously acquired jobs associated with a port
    pub async fn release_port(&self, port: u16) -> Result<()> {
        self.client
            .conn(move |conn| Self::release_by_ports(conn, once(port)))
            .await
            .context("Failed to release jobs by port in the database")?;

        Ok(())
    }

    /// Record information about a newly-acquired job
    pub async fn initialize_job(
        &self,
        name: String,
        command: String,
        next_run: Option<&DateTime<Utc>>,
    ) -> Result<()> {
        let next_run = next_run.map(chrono::DateTime::naive_utc);
        self.client
            .conn(move |conn| {
                conn.execute(
                    "UPDATE job
SET command = ?1, next_run = ?2, initialized = TRUE
WHERE name = ?3",
                    (command, next_run, name),
                )
            })
            .await
            .context("Failed to release jobs by port in the database")?;

        Ok(())
    }

    /// Return all running, initialized jobs, including jobs from other processes
    /// Jobs that have been acquired but not initialized yet are not included
    pub async fn get_active_jobs(
        &self,
        check_port_active: impl Fn(u16) -> bool + Send + 'static,
    ) -> Result<Vec<Job>> {
        // Data integrity: use an exclusive transaction to ensure that the job and port associations are not modified
        // while we are determining which jobs are active and/or recoverable
        let jobs = self
            .client
            .conn(move |conn| {
                conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION")?;

                let mut statement = conn.prepare(
                    "SELECT DISTINCT(port)
FROM job
WHERE port IS NOT NULL",
                )?;
                let ports = statement
                    .query_map((), |row| row.get("port"))?
                    .collect::<rusqlite::Result<Vec<u16>>>()?;
                let inactive_ports = ports
                    .into_iter()
                    .filter(|port| !check_port_active(*port))
                    .collect::<Vec<_>>();
                if !inactive_ports.is_empty() {
                    conn.execute(
                        "UPDATE job
SET port = NULL
WHERE port IN rarray(?1)",
                        (Self::make_rarray(inactive_ports),),
                    )?;
                }

                // Data integrity: released and uninitialized jobs are ignored
                let mut statement = conn.prepare(
                    "
WITH current_runs AS (
    SELECT id, job_name
    FROM run r
    WHERE state = 'running'
    AND id = (
        SELECT MAX(id)
        FROM run
        WHERE job_name = r.job_name
    )
)
SELECT name, command, next_run, current_runs.id IS NOT NULL AS running
FROM job
LEFT JOIN current_runs ON current_runs.job_name = job.name
WHERE port IS NOT NULL AND initialized = TRUE
ORDER BY name",
                )?;
                let jobs = statement
                    .query_map((), Job::from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;

                conn.execute_batch("COMMIT")?;

                Ok(jobs)
            })
            .await
            .context("Failed to release jobs by port in the database")?;

        Ok(jobs)
    }

    /// Release the jobs with the given names
    /// Data integrity: released jobs have no running runs
    fn release_by_names(conn: &Connection, jobs: Vec<String>) -> rusqlite::Result<()> {
        let jobs = Self::make_rarray(jobs);
        conn.execute(
            "UPDATE job
SET port = NULL
WHERE name IN rarray(?1)",
            (Rc::clone(&jobs),),
        )?;

        conn.execute(
            "UPDATE run
SET state = 'completed'
WHERE job_name IN rarray(?1)",
            (jobs,),
        )?;

        Ok(())
    }

    /// Release the jobs associated with any of the given ports
    /// Data integrity: released jobs have no running runs
    fn release_by_ports<I>(conn: &Connection, ports: I) -> rusqlite::Result<()>
    where
        I: IntoIterator<Item = u16>,
    {
        let jobs = conn
            .prepare(
                "UPDATE job
SET port = NULL
WHERE port IN rarray(?1)
RETURNING name",
            )?
            .query_map((Self::make_rarray(ports),), |row| row.get("name"))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        conn.execute(
            "UPDATE run
SET state = 'completed'
WHERE job_name IN rarray(?1)",
            (Self::make_rarray(jobs),),
        )?;

        Ok(())
    }

    /// Convert an iterator into an rarray
    fn make_rarray<T, I>(iter: I) -> Rc<Vec<Value>>
    where
        I: IntoIterator<Item = T>,
        T: Into<Value>,
    {
        Rc::new(iter.into_iter().map(Into::into).collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use tokio::test;

    use super::*;

    async fn open_db() -> Database {
        let client = ClientBuilder::new().open().await.unwrap();
        let db = Database { client };
        db.init().await.unwrap();
        db
    }

    async fn initialize_job(db: &Database, name: String) {
        db.initialize_job(name, String::new(), None).await.unwrap();
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

    fn check_port_active(_: u16) -> bool {
        true
    }

    #[test]
    async fn test_insert_run() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.acquire_jobs(vec![name.clone()], 1000, check_port_active)
            .await
            .unwrap();
        insert_run(&db, name.clone()).await;

        let runs = db.get_last_runs(name, 1).await.unwrap();
        let run = runs.first().unwrap();
        assert_eq!(run.ended_at, None);
        assert_eq!(run.status().unwrap(), RunStatus::Running { pid: 0 });
    }

    #[test]
    async fn test_insert_run_updates_job() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.acquire_jobs(vec![name.clone()], 1000, check_port_active)
            .await
            .unwrap();
        let next_run = Utc::now();
        db.initialize_job(name.clone(), String::new(), Some(&next_run))
            .await
            .unwrap();
        assert_eq!(
            db.get_active_jobs(check_port_active)
                .await
                .unwrap()
                .first()
                .unwrap()
                .status,
            JobStatus::Waiting(next_run),
        );

        insert_run(&db, name.clone()).await;

        assert_eq!(
            db.get_active_jobs(check_port_active)
                .await
                .unwrap()
                .first()
                .unwrap()
                .status,
            JobStatus::Running,
        );
    }

    #[test]
    async fn test_complete_run() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.acquire_jobs(vec![name.clone()], 1000, check_port_active)
            .await
            .unwrap();
        db.initialize_job(name.clone(), String::new(), None)
            .await
            .unwrap();
        insert_run(&db, name.clone()).await;
        db.complete_run(name.clone(), Some(0), None).await.unwrap();

        let runs = db.get_last_runs(name, 1).await.unwrap();
        let run = runs.first().unwrap();
        assert!(run.ended_at.is_some());
        assert_eq!(
            run.status().unwrap(),
            RunStatus::Completed { status_code: 0 },
        );

        assert_eq!(
            db.get_active_jobs(check_port_active)
                .await
                .unwrap()
                .first()
                .unwrap()
                .status,
            JobStatus::Completed,
        );
    }

    #[test]
    async fn test_get_last_runs() {
        let db = open_db().await;
        let name = "job".to_owned();
        db.acquire_jobs(vec![name.clone()], 1000, check_port_active)
            .await
            .unwrap();
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
        assert_eq!(
            runs[1].status().unwrap(),
            RunStatus::Completed { status_code: 0 },
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
        db.acquire_jobs(vec![name.clone()], 1000, check_port_active)
            .await
            .unwrap();
        insert_run(&db, name.clone()).await;
        // Release the job without completing the run
        db.release_jobs(vec![name.clone()]).await.unwrap();

        let runs = db.get_last_runs(name.clone(), 1).await.unwrap();
        assert_eq!(runs[0].status().unwrap(), RunStatus::Terminated);
    }

    #[test]
    async fn get_acquired_job_port() {
        let db = open_db().await;
        db.acquire_jobs(vec!["job1".to_owned()], 1000, check_port_active)
            .await
            .unwrap();
        assert_eq!(
            db.get_job_port("job1".to_owned()).await.unwrap(),
            Some(1000),
        );
    }

    #[test]
    async fn get_released_job_port() {
        let db = open_db().await;
        db.acquire_jobs(vec!["job1".to_owned()], 1000, check_port_active)
            .await
            .unwrap();
        db.release_jobs(vec!["job1".to_owned()]).await.unwrap();
        assert_eq!(db.get_job_port("job1".to_owned()).await.unwrap(), None);
    }

    #[test]
    async fn get_unknown_job_port() {
        let db = open_db().await;
        assert_eq!(db.get_job_port("job1".to_owned()).await.unwrap(), None);
    }

    #[test]
    async fn test_acquire_jobs() {
        let db = open_db().await;
        assert_eq!(
            db.acquire_jobs(
                vec!["job1".to_owned(), "job2".to_owned()],
                1000,
                check_port_active,
            )
            .await
            .unwrap(),
            ReserveResult::Reserved
        );
        assert_eq!(
            db.acquire_jobs(
                vec!["job1".to_owned(), "job2".to_owned(), "job3".to_owned()],
                1001,
                check_port_active
            )
            .await
            .unwrap(),
            ReserveResult::Failed {
                conflicting_jobs: vec!["job1".to_owned(), "job2".to_owned()]
            }
        );
        assert_eq!(
            db.acquire_jobs(vec!["job3".to_owned()], 1000, check_port_active)
                .await
                .unwrap(),
            ReserveResult::Reserved
        );
    }

    #[test]
    async fn test_acquire_jobs_uninitializes() {
        let db = open_db().await;

        // Seed with an old run and reacquire
        db.acquire_jobs(vec!["job1".to_owned()], 1000, check_port_active)
            .await
            .unwrap();
        initialize_job(&db, "job1".to_owned()).await;
        insert_run(&db, "job1".to_owned()).await;
        db.acquire_jobs(vec!["job1".to_owned()], 1000, check_port_active)
            .await
            .unwrap();

        // The job should not active be because it is uninitialized
        assert!(
            db.get_active_jobs(check_port_active)
                .await
                .unwrap()
                .is_empty(),
        );
    }

    #[test]
    async fn test_initialize_job() {
        let db = open_db().await;
        db.acquire_jobs(vec!["job1".to_owned()], 1000, check_port_active)
            .await
            .unwrap();
        db.initialize_job("job1".to_owned(), "echo Hello".to_owned(), None)
            .await
            .unwrap();

        assert_eq!(
            db.get_active_jobs(check_port_active).await.unwrap(),
            vec![Job {
                name: "job1".to_owned(),
                command: "echo Hello".to_owned(),
                status: JobStatus::Completed,
            }]
        );
    }

    #[test]
    async fn test_reacquire_jobs() {
        let db = open_db().await;
        assert_eq!(
            db.acquire_jobs(
                vec!["job1".to_owned(), "job2".to_owned()],
                1000,
                check_port_active,
            )
            .await
            .unwrap(),
            ReserveResult::Reserved
        );
        insert_run(&db, "job1".to_owned()).await;
        insert_run(&db, "job2".to_owned()).await;
        assert_eq!(
            db.acquire_jobs(
                vec!["job1".to_owned(), "job2".to_owned(), "job3".to_owned()],
                1000,
                check_port_active
            )
            .await
            .unwrap(),
            ReserveResult::Reserved
        );
    }

    #[test]
    async fn test_acquire_recoverable_jobs() {
        let db = open_db().await;
        assert_eq!(
            db.acquire_jobs(
                vec!["job1".to_owned(), "job2".to_owned()],
                1000,
                check_port_active,
            )
            .await
            .unwrap(),
            ReserveResult::Reserved
        );
        assert_eq!(
            db.acquire_jobs(vec!["job3".to_owned()], 1001, check_port_active)
                .await
                .unwrap(),
            ReserveResult::Reserved
        );
        insert_run(&db, "job1".to_owned()).await;
        insert_run(&db, "job2".to_owned()).await;
        insert_run(&db, "job3".to_owned()).await;

        // Port 1001 is inactive and job 3 is released
        assert_eq!(
            db.acquire_jobs(
                vec!["job1".to_owned(), "job2".to_owned(), "job3".to_owned()],
                1002,
                |port| port == 1000
            )
            .await
            .unwrap(),
            ReserveResult::Failed {
                conflicting_jobs: vec!["job1".to_owned(), "job2".to_owned()]
            }
        );

        assert_eq!(
            db.acquire_jobs(vec!["job3".to_owned()], 1000, check_port_active)
                .await
                .unwrap(),
            ReserveResult::Reserved
        );
    }

    #[test]
    async fn test_acquire_released_jobs() {
        let db = open_db().await;
        let jobs = vec!["job1".to_owned(), "job2".to_owned()];
        assert_eq!(
            db.acquire_jobs(jobs.clone(), 1000, check_port_active)
                .await
                .unwrap(),
            ReserveResult::Reserved
        );
        db.release_jobs(jobs.clone()).await.unwrap();
        assert_eq!(
            db.acquire_jobs(jobs, 1000, check_port_active)
                .await
                .unwrap(),
            ReserveResult::Reserved
        );
    }

    #[test]
    async fn test_acquire_jobs_empty() {
        let db = open_db().await;
        assert_eq!(
            db.acquire_jobs(Vec::new(), 1000, check_port_active)
                .await
                .unwrap(),
            ReserveResult::Reserved
        );
    }

    #[test]
    async fn test_release_jobs() {
        let db = open_db().await;
        assert_eq!(
            db.acquire_jobs(
                vec!["job1".to_owned(), "job2".to_owned()],
                1000,
                check_port_active,
            )
            .await
            .unwrap(),
            ReserveResult::Reserved
        );
        db.release_jobs(vec!["job2".to_owned()]).await.unwrap();
        assert_eq!(
            db.acquire_jobs(
                vec!["job2".to_owned(), "job3".to_owned()],
                1000,
                check_port_active,
            )
            .await
            .unwrap(),
            ReserveResult::Reserved
        );
    }

    #[test]
    async fn test_release_jobs_empty() {
        let db = open_db().await;
        db.release_jobs(Vec::new()).await.unwrap();
    }

    #[test]
    async fn test_release_port() {
        let db = open_db().await;
        let jobs = vec!["job1".to_owned(), "job2".to_owned()];
        assert_eq!(
            db.acquire_jobs(jobs.clone(), 1000, check_port_active)
                .await
                .unwrap(),
            ReserveResult::Reserved
        );

        db.release_port(1000).await.unwrap();

        assert_eq!(
            db.acquire_jobs(jobs, 1001, check_port_active)
                .await
                .unwrap(),
            ReserveResult::Reserved
        );
    }

    #[test]
    async fn test_get_active_jobs_uninitialized() {
        let db = open_db().await;
        db.acquire_jobs(
            vec!["job1".to_owned(), "job2".to_owned()],
            1000,
            check_port_active,
        )
        .await
        .unwrap();

        assert_eq!(db.get_active_jobs(check_port_active).await.unwrap(), vec![]);
    }

    #[test]
    async fn test_get_active_jobs_partially_initialized() {
        let db = open_db().await;
        db.acquire_jobs(
            vec!["job1".to_owned(), "job2".to_owned()],
            1000,
            check_port_active,
        )
        .await
        .unwrap();
        initialize_job(&db, "job1".to_owned()).await;

        assert_eq!(
            db.get_active_jobs(check_port_active)
                .await
                .unwrap()
                .into_iter()
                .map(|job| job.name)
                .collect::<Vec<_>>(),
            vec!["job1".to_owned()]
        );
    }

    #[test]
    async fn test_get_active_jobs_released() {
        let db = open_db().await;
        db.acquire_jobs(
            vec!["job1".to_owned(), "job2".to_owned()],
            1000,
            check_port_active,
        )
        .await
        .unwrap();
        initialize_job(&db, "job1".to_owned()).await;
        db.release_jobs(vec!["job1".to_owned()]).await.unwrap();

        assert_eq!(db.get_active_jobs(check_port_active).await.unwrap(), vec![]);
    }

    #[test]
    async fn test_get_active_jobs_inactive() {
        let db = open_db().await;
        let jobs = vec!["job1".to_owned(), "job2".to_owned()];
        db.acquire_jobs(jobs.clone(), 1000, check_port_active)
            .await
            .unwrap();
        db.acquire_jobs(vec!["job3".to_owned()], 1001, check_port_active)
            .await
            .unwrap();
        initialize_job(&db, "job1".to_owned()).await;
        initialize_job(&db, "job2".to_owned()).await;
        initialize_job(&db, "job3".to_owned()).await;

        assert_eq!(
            db.get_active_jobs(|port| port == 1000)
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

        db.acquire_jobs(vec!["job1".to_owned()], 1000, check_port_active)
            .await
            .unwrap();
        // The job should not active be because it has not been initialized yet
        assert!(
            db.get_active_jobs(check_port_active)
                .await
                .unwrap()
                .is_empty(),
        );

        initialize_job(&db, "job1".to_owned()).await;
        // The job should not be running because it does not have an active run yet
        assert_eq!(
            db.get_active_jobs(check_port_active)
                .await
                .unwrap()
                .first()
                .unwrap()
                .status,
            JobStatus::Completed,
        );

        insert_run(&db, "job1".to_owned()).await;
        assert_eq!(
            db.get_active_jobs(check_port_active)
                .await
                .unwrap()
                .first()
                .unwrap()
                .status,
            JobStatus::Running,
        );
    }
}
