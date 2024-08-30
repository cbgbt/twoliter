#[cfg_attr(target_os = "linux", path = "link.rs")]
#[cfg_attr(not(target_os = "linux"), path = "non_linux_link.rs")]
pub(crate) mod link;

#[cfg_attr(target_os = "linux", path = "multi_link.rs")]
#[cfg_attr(not(target_os = "linux"), path = "non_linux_multi_link.rs")]
mod multi_link;

use self::link::Link;
use self::multi_link::MultiLink;
use pipesys::multi_server::MultiServerArgs as MultiServe;
use pipesys::server::Server as Serve;

use anyhow::Result;
#[cfg(target_os = "linux")]
use anyhow::{anyhow, bail, ensure, Context};
use clap::Parser;
#[cfg(target_os = "linux")]
use log::debug;
use log::LevelFilter;
#[cfg(target_os = "linux")]
use nix::fcntl::{fcntl, F_DUPFD};
#[cfg(target_os = "linux")]
use std::path::PathBuf;

const DEFAULT_LEVEL_FILTER: LevelFilter = LevelFilter::Info;

/// A tool for sharing file descriptors over abstract Unix domain sockets.
#[derive(Debug, Parser)]
#[clap(about, long_about = None, version)]
pub(crate) struct Args {
    /// Set the logging level. One of [off|error|warn|info|debug|trace]. Defaults to warn. You can
    /// also leave this unset and use the RUST_LOG env variable. See
    /// https://github.com/rust-cli/env_logger/
    #[clap(long = "log-level")]
    pub(crate) log_level: Option<LevelFilter>,

    #[clap(subcommand)]
    pub(crate) subcommand: Subcommand,
}

#[derive(Debug, Parser)]
pub(crate) enum Subcommand {
    /// Serve file descriptors to clients.
    Serve(Serve),

    /// Link a directory file descriptor to the target path.
    Link(Link),

    MultiServe(MultiServe),
    MultiLink(MultiLink),
}

/// Entrypoint for the `pipesys` command line program.
pub(super) async fn run(args: Args) -> Result<()> {
    match args.subcommand {
        Subcommand::Serve(serve_args) => serve_args.serve().await,
        Subcommand::Link(link_args) => link_args.execute().await,
        Subcommand::MultiServe(multi_serve_args) => multi_serve_args.serve().await,
        Subcommand::MultiLink(multi_serve_args) => multi_serve_args.execute().await,
    }
}

/// use `level` if present, or else use `RUST_LOG` if present, or else use a default.
pub(super) fn init_logger(level: Option<LevelFilter>) {
    match (std::env::var(env_logger::DEFAULT_FILTER_ENV).ok(), level) {
        (Some(_), None) => {
            // RUST_LOG exists and level does not; use the environment variable.
            env_logger::Builder::from_default_env().init();
        }
        _ => {
            // use provided log level or default for this crate only.
            env_logger::Builder::new()
                .filter(
                    Some(env!("CARGO_CRATE_NAME")),
                    level.unwrap_or(DEFAULT_LEVEL_FILTER),
                )
                .init();
        }
    }
}

// Don't accept file descriptors 0, 1, or 2 since those correspond to the well-known stdin, stdout,
// and stderr which could confuse the calling process or its children.
#[cfg(target_os = "linux")]
const MIN_FD: i32 = 3;

/// Helper function to retrieve a file descriptor via an abstract socket.
#[cfg(target_os = "linux")]
fn fetch_fd(socket: &str) -> Result<i32> {
    let addr = uds::UnixSocketAddr::from_abstract(socket.as_bytes())
        .with_context(|| format!("failed to create socket {}", socket))?;
    let client = uds::UnixSeqpacketConn::connect_unix_addr(&addr)
        .with_context(|| format!("failed to connect to socket {}", socket))?;

    let mut fd_buf = [-1; 1];
    let (_, _, fds) = client
        .recv_fds(&mut [0u8; 1], &mut fd_buf)
        .with_context(|| format!("failed to receive file descriptor from socket {}", socket))?;

    ensure!(
        fds == 1,
        format!("received {fds} file descriptors, expected 1")
    );

    let fd = fd_buf
        .first()
        .filter(|fd| **fd >= MIN_FD)
        .with_context(|| {
            format!(
                "did not receive valid file descriptor from socket {}",
                socket
            )
        })?;

    let dupfd =
        duplicate_fd(*fd).with_context(|| format!("failed to duplicate file descriptor {fd}"))?;
    debug!("duplicated file descriptor {fd} to {dupfd}");

    Ok(dupfd)
}

/// Helper function to retrieve a usize from a unix socket.
#[cfg(target_os = "linux")]
fn fetch_usize(
    socket: &str,
    socket_client: &uds::UnixSeqpacketConn,
    field_name: &str,
) -> Result<usize> {
    let mut usize_buff = [0u8; std::mem::size_of::<usize>()];
    if usize_buff.len()
        != socket_client
            .recv(&mut usize_buff)
            .with_context(|| format!("failed to receive '{}' from socket {}", field_name, socket))?
    {
        bail!("socket sent invalid '{field_name}' {usize_buff:?}");
    }
    Ok(usize::from_ne_bytes(usize_buff))
}

/// Helper function to retrieve a file descriptor via an abstract socket.
#[cfg(target_os = "linux")]
fn fetch_fds(socket: &str) -> Result<Vec<(PathBuf, i32)>> {
    let addr = uds::UnixSocketAddr::from_abstract(socket.as_bytes())
        .with_context(|| format!("failed to create socket {}", socket))?;
    let client = uds::UnixSeqpacketConn::connect_unix_addr(&addr)
        .with_context(|| format!("failed to connect to socket {}", socket))?;

    let targets_message_len = fetch_usize(socket, &client, "targets message length")?;
    let num_fds = fetch_usize(socket, &client, "number of file descriptors")?;

    let mut fd_buf = vec![-1; num_fds];
    let mut targets_message_buf = vec![0u8; targets_message_len];

    let (bytes, truncated, fds) = client
        .recv_fds(&mut targets_message_buf, &mut fd_buf)
        .with_context(|| format!("failed to receive file descriptor from socket {socket}"))?;

    ensure!(
        fds == num_fds,
        format!("received {fds} file descriptors, expected {num_fds}")
    );
    ensure!(
        bytes == targets_message_len,
        format!("received {bytes} bytes for targets, expected {targets_message_len}")
    );
    ensure!(
        !truncated,
        format!(
            "expected {targets_message_len} bytes for targets, but more received on socket {socket}"
        )
    );

    let targets: Vec<PathBuf> = bincode::deserialize(&targets_message_buf)
        .with_context(|| format!("failed to deserialize targets from socket {socket}"))?;

    let fds: Vec<i32> = fd_buf
        .into_iter()
        .map(|fd| {
            (fd > MIN_FD)
                .then_some(fd)
                .ok_or_else(|| {
                    anyhow!("received invalid file descriptor {fd} from socket {socket}")
                })
                .and_then(duplicate_fd)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(targets.into_iter().zip(fds.into_iter()).collect())
}

/// Duplicate file descriptors without the CLOEXEC flag set.
#[cfg(target_os = "linux")]
fn duplicate_fd(fd: i32) -> Result<i32> {
    let newfd = fcntl(fd, F_DUPFD(MIN_FD))
        .with_context(|| format!("failed to duplicate file descriptor {fd}"))?;
    debug!("duplicated file descriptor {fd} to {newfd}");
    Ok(newfd)
}
