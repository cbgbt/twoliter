use super::link::{inotify_init, inotify_wait, output_streams, parent_dir};
use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use daemonize::{Daemonize, Outcome};
use inotify::WatchMask;
use log::{error, info, trace};
use std::path::{Path, PathBuf};
use std::process;
use tokio::fs;

use crate::cmd::fetch_fds;

#[derive(Debug, Parser)]
pub(crate) struct MultiLink {
    /// Fetch the file descriptors from this abstract socket.
    #[clap(long = "fd-socket")]
    fd_socket: String,

    /// Create symlinks under this parent path.
    #[clap(long = "parent")]
    parent: PathBuf,

    /// Use deletion of this marker file to indicate that the links should be removed.
    #[clap(long = "marker")]
    marker: PathBuf,
}

impl MultiLink {
    pub(crate) async fn execute(&self) -> Result<()> {
        if self.parent.exists() {
            bail!(
                "found existing file or directory at {}",
                self.parent.display()
            );
        }
        if self.marker.exists() {
            bail!(
                "found existing file or directory at {}",
                self.marker.display()
            );
        }

        // Retrieve the file descriptors to link
        let fd_map = fetch_fds(&self.fd_socket)?;

        // Create a log file for the background process.
        let parent_dir = parent_dir(&self.parent)?;
        let log_file = parent_dir.join("pipesys-link.log");
        let (stdout, stderr) = output_streams(&log_file).await.with_context(|| {
            format!("failed to create output streams for {}", log_file.display())
        })?;

        // After we daemonize, we need to avoid returning back to the caller since the async
        // runtime and associated state are no longer valid.
        std::thread::scope(|s| {
            s.spawn(|| {
                if let Outcome::Child(res) = Daemonize::new()
                    .stdout(stdout)
                    .stderr(stderr)
                    .working_directory(parent_dir)
                    .execute()
                {
                    if let Err(e) = res {
                        error!("failed to daemonize: {e}");
                        std::process::exit(1);
                    }

                    trace!("daemonized!");
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_io()
                        .build()
                        .expect("failed to build runtime");

                    rt.block_on(async {
                        if let Err(e) = self.manage_symlinks(fd_map).await {
                            error!("failed to manage file descriptors: {e}");
                            std::process::exit(1);
                        }
                    });

                    info!("done");
                    std::process::exit(0);
                }
            });
        });

        self.wait_for_marker().await
    }

    async fn wait_for_marker(&self) -> Result<()> {
        let inotify = inotify_init(&self.marker, WatchMask::CREATE)?;
        inotify_wait(inotify, &self.marker, &file_found).await
    }

    async fn manage_symlinks(&self, fd_map: Vec<(PathBuf, i32)>) -> Result<()> {
        let inotify_marker_create = inotify_init(&self.marker, WatchMask::CREATE)?;
        let inotify_marker_delete = inotify_init(&self.marker, WatchMask::DELETE)?;

        let pid = process::id();
        for (path, fd) in fd_map {
            let link = as_relative_path(&self.parent, &path)?;
            let parent = link.parent().with_context(|| {
                format!("failed to get parent directory for {}", link.display())
            })?;
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create {}", parent.display()))?;

            let source = format!("/proc/{pid}/fd/{fd}");
            fs::symlink(&source, &link)
                .await
                .with_context(|| format!("failed to create symlink at {}", link.display()))?;
            info!("symlinked {} to {source}", link.display());
        }

        fs::write(&self.marker, b"")
            .await
            .with_context(|| format!("failed to create marker file {}", self.marker.display()))?;

        inotify_wait(inotify_marker_create, &self.marker, &file_found).await?;
        inotify_wait(inotify_marker_delete, &self.marker, &file_not_found).await?;

        fs::remove_dir_all(&self.parent)
            .await
            .with_context(|| format!("failed to remove parent dir {}", self.parent.display()))
    }
}

/// Returns true if a file exists at the path, and false otherwise.
pub(crate) async fn file_found(path: &Path) -> bool {
    let res = fs::metadata(path).await.is_ok();

    if res {
        trace!("found file for {}", path.display());
    } else {
        trace!("no file found for {}", path.display());
    }

    res
}

/// Returns false if a file exists at the path, and true otherwise.
pub(crate) async fn file_not_found(path: &Path) -> bool {
    !file_found(path).await
}

fn as_relative_path(parent: impl AsRef<Path>, path: impl AsRef<Path>) -> Result<PathBuf> {
    let parent = parent.as_ref();
    let path = path.as_ref();

    if path.components().find(|c| c.as_os_str() == "..").is_some() {
        bail!(
            "target path {} may not refer to parents as '..'",
            path.display()
        );
    }

    let relative_path = path.strip_prefix("/").unwrap_or(&path);
    ensure!(
        !relative_path.starts_with(".."),
        "input path {} is outside of the bouds of the provided parent {}",
        path.display(),
        parent.display()
    );

    Ok(parent.join(relative_path))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_as_relative_path_with_absolute_input() {
        // Even if `path` is absolute, we make it relative to `parent`
        let parent = "/tmp";
        let path = "/tmp/foo/bar";
        let expected = "/tmp/tmp/foo/bar";

        let result = as_relative_path(parent, path).unwrap();
        assert_eq!(result, PathBuf::from(expected));
    }

    #[test]
    fn test_as_relative_path_with_relative_parent() {
        // Even if `path` is absolute, we make it relative to `parent`
        let parent = "tmp";
        let path = "/and/then";
        let expected = "tmp/and/then";

        let result = as_relative_path(parent, path).unwrap();
        assert_eq!(result, PathBuf::from(expected));
    }

    #[test]
    fn test_as_relative_path_would_escape() {
        // Even if `path` is absolute, we make it relative to `parent`
        let parent = "/anything";
        let path = "/uh-oh/../../../../help";

        assert!(as_relative_path(parent, path).is_err());
    }
}
