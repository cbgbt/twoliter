use anyhow::{Context, Result};
use clap::Parser;
use log::warn;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uds::{tokio::UnixSeqpacketListener, UnixSocketAddr};

/// Serve the file descriptor for a path over an abstract UNIX domain socket.
#[derive(Clone, Debug, Parser)]
pub struct MultiServerArgs {
    /// Listen on this abstract socket.
    #[clap(long = "socket")]
    socket: String,

    /// Expect clients with this UID.
    #[clap(long = "client-uid")]
    client_uid: u32,

    /// Read file descriptor config from this path.
    #[clap(long = "config-path")]
    config_path: PathBuf,
}

impl MultiServerArgs {
    pub async fn serve(&self) -> Result<()> {
        let conf_str = tokio::fs::read_to_string(&self.config_path)
            .await
            .with_context(|| {
                format!(
                    "failed to read server config from {}",
                    self.config_path.display()
                )
            })?;
        let config = serde_json::from_str(&conf_str).with_context(|| {
            format!(
                "failed to parse server config from {}",
                self.config_path.display()
            )
        })?;

        // Start the server
        let server = MultiServer::new(self.socket.clone(), self.client_uid, config).await?;
        server.serve().await
    }
}

#[derive(Clone, Debug)]
pub struct MultiServer {
    socket: String,
    client_uid: u32,
    config: MultiServerConf,
}

impl MultiServer {
    pub async fn new<S>(socket: S, client_uid: u32, config: MultiServerConf) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let socket = socket.as_ref().to_string();
        Ok(Self {
            socket,
            client_uid,
            config,
        })
    }

    pub async fn serve(&self) -> Result<()> {
        let addr = UnixSocketAddr::from_abstract(self.socket.as_bytes())
            .with_context(|| format!("failed to create socket {}", self.socket))?;
        let mut listener = UnixSeqpacketListener::bind_addr(&addr)
            .with_context(|| format!("failed to bind to socket {}", self.socket))?;

        let source_files = self
            .config
            .file_bindings()
            .iter()
            .map(|binding| {
                let source_file = OpenOptions::new()
                    .create(false)
                    .read(true)
                    .write(false)
                    .open(binding.source_path())
                    .with_context(|| {
                        format!("could not open {}", binding.source_path().display())
                    })?;
                let fd = source_file.as_raw_fd();

                // We need to keep the files around to keep them open
                Ok((source_file, fd))
            })
            .collect::<Result<Vec<_>>>()?;

        let fds = source_files.iter().map(|(_, fd)| *fd).collect::<Vec<_>>();

        let target_paths = self
            .config
            .file_bindings()
            .iter()
            .map(FileBinding::target_path)
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();

        let target_paths = bincode::serialize(&target_paths)
            .with_context(|| format!("failed to serialize target paths as bincode"))?;

        let socket = Arc::new(self.socket.clone());
        let target_paths = Arc::new(target_paths);
        let fds = Arc::new(fds);
        loop {
            let (mut conn, _) = listener
                .accept()
                .await
                .with_context(|| format!("failed to accept connection on socket {}", socket))?;

            let peer_creds = conn.initial_peer_credentials().with_context(|| {
                format!(
                    "failed to obtain peer credentials on socket {}",
                    self.socket
                )
            })?;

            let peer_uid = peer_creds.euid();
            if peer_uid != self.client_uid {
                warn!("ignoring connection from peer with UID {}", peer_uid);
                continue;
            }

            let socket = Arc::clone(&socket);
            let target_paths = Arc::clone(&target_paths);
            let fds = Arc::clone(&fds);
            tokio::spawn(async move {
                let targets_msg_len: usize = target_paths.len();
                let num_fds: usize = fds.len();

                conn.send(&targets_msg_len.to_ne_bytes())
                    .await
                    .with_context(|| {
                        format!("failed to send targets message length over {}", socket)
                    })?;
                conn.send(&num_fds.to_ne_bytes())
                    .await
                    .with_context(|| format!("failed to send number of fds over {}", socket))?;
                conn.send_fds(&target_paths, &fds)
                    .await
                    .with_context(|| format!("failed to send file descriptors over {}", socket))
            });
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct MultiServerConf {
    file_bindings: Vec<FileBinding>,
}

impl MultiServerConf {
    pub fn new(file_bindings: Vec<FileBinding>) -> Self {
        Self { file_bindings }
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let f = OpenOptions::new()
            .read(true)
            .open(path.as_ref())
            .with_context(|| format!("could not open {}", path.as_ref().display()))?;

        serde_json::from_reader(f)
            .with_context(|| format!("failed to parse {}", path.as_ref().display()))
    }

    pub fn file_bindings(&self) -> &[FileBinding] {
        &self.file_bindings
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct FileBinding {
    source_path: PathBuf,
    target_path: PathBuf,
}

impl FileBinding {
    pub fn new(source_path: PathBuf, target_path: PathBuf) -> Self {
        Self {
            source_path,
            target_path,
        }
    }

    /// Path to the source file (the file to serve
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub fn target_path(&self) -> &Path {
        &self.target_path
    }
}
