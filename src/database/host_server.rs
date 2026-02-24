use super::ipc::{self, Request, Response};
use crate::chron_service::ChronService;
use anyhow::{Context, Result, anyhow};
use interprocess::local_socket::{
    GenericFilePath, ListenerOptions, ToFsName,
    tokio::{Listener, RecvHalf, SendHalf, prelude::*},
    traits::tokio::Stream,
};
use log::info;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
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
    ipc_listener: RwLock<Option<Listener>>,
    server_handle: RwLock<Option<ServerHandle>>,
}

impl HostServer {
    /// Create a host server for a chron directory
    /// A host server ensures that only one chron host is running at a time and forwards messages from clients to the
    /// host's chron service.
    pub async fn new(chron_dir: &Path) -> Result<Self> {
        let socket_path = Self::get_socket_path(chron_dir);
        let name = socket_path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .context("Failed to create socket name")?;
        let listener = match ListenerOptions::new().name(name.clone()).create_tokio() {
            Ok(listener) => listener,
            Err(err) => {
                // An ungraceful shutdown can leave behind a stale socket file. Try connecting to determine whether a
                // host server is actually rerunning. If not, clean up the socket file and retry.
                if err.kind() == std::io::ErrorKind::AddrInUse
                    && Self::connect(chron_dir).await.is_err()
                {
                    info!("Cleaning up stale socket");
                    tokio::fs::remove_file(&socket_path).await?;
                    ListenerOptions::new().name(name).create_tokio()
                } else {
                    Err(err)
                }
                .map_err(|err| {
                    if err.kind() == std::io::ErrorKind::AddrInUse {
                        anyhow!("chron is already running")
                    } else {
                        err.into()
                    }
                })?
            }
        };

        Ok(Self {
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
            loop {
                let result = tokio::select! {
                    () = token.cancelled() => break,
                    result = listener.accept() => result
                };
                let Ok(conn) = result else { break };
                let (mut rx, mut tx) = conn.split();
                let chron = Arc::clone(&chron);
                let token = token.child_token();
                tokio::spawn(async move {
                    loop {
                        let req = tokio::select! {
                            () = token.cancelled() => return Ok::<(), anyhow::Error>(()),
                            req = ipc::receive::<Request, _>(&mut rx) => req?,
                        };
                        let res = match req {
                            Request::Connect => Response::Connect,
                            Request::Trigger { name } => {
                                let mut chron_lock = chron.write().await;
                                let result = chron_lock.trigger(&name).await;
                                drop(chron_lock);
                                Response::Trigger { result }
                            }
                            Request::Terminate { name } => {
                                let chron_lock = chron.read().await;
                                let pid = match chron_lock.get_job(&name) {
                                    Some(job) => job.terminate().await,
                                    None => None,
                                };
                                drop(chron_lock);
                                Response::Terminate { pid }
                            }
                        };
                        ipc::send(&mut tx, &res).await?;
                    }
                });
            }
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
        Ok(())
    }

    /// Connect with a host and return its port
    pub(super) async fn connect(chron_dir: &Path) -> Result<(RecvHalf, SendHalf)> {
        let path = Self::get_socket_path(chron_dir);
        let name = path.as_path().to_fs_name::<GenericFilePath>()?;
        Ok(LocalSocketStream::connect(name).await?.split())
    }

    /// Return the location of the socket file that clients use to communicate with the host
    fn get_socket_path(chron_dir: &Path) -> PathBuf {
        chron_dir.join("host.sock")
    }
}
