use super::db::Database;
use super::ipc::{self, Request, Response, TriggerResult};
use super::models::{Job, Run, RunStatus};
use crate::database::HostServer;
use anyhow::{Result, bail};
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use std::path::Path;

pub struct ClientDatabase {
    db: Database,
    tx: SendHalf,
    rx: RecvHalf,
}

impl ClientDatabase {
    /// Open the database as a read-only client
    /// The database will be able to read job information from an open `Host`.
    pub async fn open(chron_dir: &Path) -> Result<Self> {
        let (mut rx, mut tx) = HostServer::connect(chron_dir).await?;
        let req = Request::Connect;
        ipc::send(&mut tx, &req).await?;
        let res = ipc::receive::<Response, _>(&mut rx).await?;
        let Response::Connect = res else {
            bail!("chron is not running");
        };

        let db = Database::new(chron_dir).await?;
        Ok(Self { db, tx, rx })
    }

    /// Trigger a one-off job run
    pub async fn trigger_job(&mut self, name: &str) -> Result<TriggerResult> {
        let req = Request::Trigger {
            name: name.to_owned(),
        };
        ipc::send(&mut self.tx, &req).await?;
        let res = ipc::receive::<Response, _>(&mut self.rx).await?;
        let Response::Trigger { result } = res else {
            bail!("Unexpected response")
        };
        Ok(result)
    }

    /// Terminate a running job by name
    pub async fn terminate_job(&mut self, name: &str) -> Result<ipc::TerminateResult> {
        let req = Request::Terminate {
            name: name.to_owned(),
        };
        ipc::send(&mut self.tx, &req).await?;
        let res = ipc::receive::<Response, _>(&mut self.rx).await?;
        let Response::Terminate { result } = res else {
            bail!("Unexpected response")
        };
        Ok(result)
    }

    pub async fn get_last_runs(&self, name: String, count: usize) -> Result<Vec<Run>> {
        self.db.get_last_runs(name, count).await
    }

    pub async fn get_run_status(&self, run_id: u32) -> Result<Option<RunStatus>> {
        self.db.get_run_status(run_id).await
    }

    pub async fn get_active_jobs(&self) -> Result<Vec<Job>> {
        self.db.get_active_jobs().await
    }

    pub async fn get_active_job(&self, name: String) -> Result<Option<Job>> {
        self.db.get_active_job(name).await
    }
}
