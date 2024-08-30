use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct MultiServerConf {
    file_bindings: Vec<FileBinding>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct FileBinding {
    source_path: PathBuf,
    target_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct MultiServer {
    socket: String,
    client_uid: u32,
    config: MultiServerConf,
}

impl MultiServer {
    pub async fn from_config<S, P>(socket: S, client_uid: u32, config_path: P) -> Result<Self>
    where
        S: AsRef<str>,
        P: AsRef<Path>,
    {
        unimplemented!()
    }

    pub async fn serve(&self) -> Result<()> {
        unimplemented!()
    }
}

impl MultiServerArgs {
    pub async fn serve(&self) -> Result<()> {
        unimplemented!()
    }
}
