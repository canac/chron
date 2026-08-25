use super::ipc::{self, Request, Response};
use crate::chron_service::ChronService;
use anyhow::{Context, Result, anyhow, bail};
use interprocess::local_socket::{
    GenericFilePath, ListenerOptions, ToFsName,
    tokio::{Listener, RecvHalf, SendHalf, prelude::*},
    traits::tokio::Stream,
};
use log::info;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::{JoinHandle, JoinSet, spawn_blocking};
use tokio_util::sync::CancellationToken;

struct ServerHandle {
    cancel_token: CancellationToken,
    handle: JoinHandle<()>,
}

impl ServerHandle {
    /// Terminate the host server
    pub async fn stop(self) -> Result<()> {
        self.cancel_token.cancel();
        Ok(self.handle.await?)
    }
}

pub struct HostServer {
    lock_file: File,
    ipc_listener: RwLock<Option<Listener>>,
    server_handle: RwLock<Option<ServerHandle>>,
}

impl HostServer {
    /// Create a host server for a chron directory
    /// A host server ensures that only one chron host is running at a time and forwards messages from clients to the
    /// host's chron service.
    pub async fn new(chron_dir: &Path) -> Result<Self> {
        let Some(lock_file) = Self::acquire_lock(chron_dir).await? else {
            bail!("chron is already running");
        };

        let socket_path = Self::get_socket_path(chron_dir);
        let name = socket_path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .context("Failed to create socket name")?;
        let listener = match ListenerOptions::new().name(name.clone()).create_tokio() {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                info!("Cleaning up stale socket");
                let temp_path =
                    socket_path.with_file_name(format!("host.{}.sock", std::process::id()));
                let temp_name = temp_path
                    .as_path()
                    .to_fs_name::<GenericFilePath>()
                    .context("Failed to create temporary socket name")?;
                let listener = ListenerOptions::new().name(temp_name).create_tokio()?;
                if let Err(err) = tokio::fs::rename(&temp_path, &socket_path).await {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return Err(err.into());
                }
                listener
            }
            Err(err) => return Err(err.into()),
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
                .await
                .context("Failed to restrict socket permissions")?;
        }

        Ok(Self {
            lock_file,
            ipc_listener: RwLock::new(Some(listener)),
            server_handle: RwLock::new(None),
        })
    }

    /// Connect the host to a chron service and begin handling client requests
    pub async fn start(&self, chron: Arc<RwLock<ChronService>>) {
        let listener = self.ipc_listener.write().await.take();
        if let Some(listener) = listener {
            *self.server_handle.write().await = Some(Self::start_server(listener, chron));
        }
    }

    /// Start the server that listens over IPC for client requests, executes them using the chron service, and sends back
    /// a response
    fn start_server(listener: Listener, chron: Arc<RwLock<ChronService>>) -> ServerHandle {
        let cancel_token = CancellationToken::new();
        let token = cancel_token.clone();
        let handle = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                let result = tokio::select! {
                    () = token.cancelled() => break,
                    Some(_) = connections.join_next() => continue,
                    result = listener.accept() => result
                };
                let Ok(conn) = result else { break };
                let (mut rx, mut tx) = conn.split();
                let chron = Arc::clone(&chron);
                let token = token.child_token();
                connections.spawn(async move {
                    loop {
                        let req = tokio::select! {
                            () = token.cancelled() => return Ok::<(), anyhow::Error>(()),
                            req = ipc::receive::<Request, _>(&mut rx) => req?,
                        };
                        let res = match req {
                            Request::Connect => Response::Connect,
                            Request::Trigger { name } => {
                                let mut chron_lock = chron.write().await;
                                let result = chron_lock.trigger(&name).await?;
                                drop(chron_lock);
                                Response::Trigger { result }
                            }
                            Request::Terminate { name } => {
                                let chron_lock = chron.read().await;
                                let result = chron_lock.terminate(&name).await;
                                drop(chron_lock);
                                Response::Terminate { result }
                            }
                        };
                        ipc::send(&mut tx, &res).await?;
                    }
                });
            }

            connections.shutdown().await;
        });

        ServerHandle {
            cancel_token,
            handle,
        }
    }

    /// Close the database, releasing it to be opened by a different host
    pub async fn close(self) -> Result<()> {
        let handle = self.server_handle.write().await.take();
        if let Some(handle) = handle {
            handle.stop().await?;
        }
        let lock_file = self.lock_file;
        asyncify(move || lock_file.unlock())
            .await
            .context("Failed to unlock the chron directory")?;
        Ok(())
    }

    /// Connect with a host and return its port
    pub(super) async fn connect(chron_dir: &Path) -> Result<(RecvHalf, SendHalf)> {
        let path = Self::get_socket_path(chron_dir);
        let name = path.as_path().to_fs_name::<GenericFilePath>()?;
        let stream = LocalSocketStream::connect(name).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound
                || err.kind() == std::io::ErrorKind::ConnectionRefused
            {
                anyhow!("chron is not running")
            } else {
                err.into()
            }
        })?;
        Ok(stream.split())
    }

    /// Return the location of the socket file that clients use to communicate with the host
    fn get_socket_path(chron_dir: &Path) -> PathBuf {
        chron_dir.join("host.sock")
    }

    /// Return the location of the lock file that enforces one host per chron directory
    fn get_lock_path(chron_dir: &Path) -> PathBuf {
        chron_dir.join("host.lock")
    }

    /// Acquire an exclusive lock on a chron directory
    async fn acquire_lock(chron_dir: &Path) -> Result<Option<File>> {
        let path = Self::get_lock_path(chron_dir);
        let lock_path = path.clone();
        asyncify(move || {
            let file = File::options()
                .create(true)
                .write(true)
                .truncate(false)
                .open(lock_path)?;
            match file.try_lock() {
                Ok(()) => Ok(Some(file)),
                Err(TryLockError::WouldBlock) => Ok(None),
                Err(TryLockError::Error(err)) => Err(err),
            }
        })
        .await
        .with_context(|| format!("Failed to lock {}", path.display()))
    }
}

/// Mirrors `tokio::fs::asyncify`
async fn asyncify<F, T>(f: F) -> std::io::Result<T>
where
    F: FnOnce() -> std::io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match spawn_blocking(f).await {
        Ok(result) => result,
        Err(err) => Err(std::io::Error::other(err)),
    }
}
