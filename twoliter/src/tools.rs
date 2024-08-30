use anyhow::{Context, Result};
use filetime::{set_file_handle_times, set_file_mtime, FileTime};
use flate2::read::ZlibDecoder;
use futures::stream;
use futures::stream::{StreamExt, TryStreamExt};
use pentacle::SealOptions;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use tar::Archive;
use tokio::fs;
use tokio::runtime::Handle;
use tracing::{debug, error};

const TAR_GZ_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tools.tar.gz"));
const BOTTLEROCKET_VARIANT: &[u8] =
    include_bytes!(env!("CARGO_BIN_FILE_BUILDSYS_bottlerocket-variant"));
const BUILDSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_BUILDSYS"));
const PIPESYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PIPESYS"));
const PUBSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PUBSYS"));
const PUBSYS_SETUP: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PUBSYS_SETUP"));
const TESTSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_TESTSYS"));
const TUFTOOL: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_TUFTOOL"));
const UNPLUG: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_UNPLUG"));

pub(crate) struct Tools<T: SealedTool> {
    sealed_tools: HashMap<PathBuf, T>,
}

impl Tools<SealedFile> {
    /// Installs tools into sealed anonymous files, using `memfd_create(2)` on Linux.
    pub(crate) async fn install() -> Result<Tools<SealedFile>> {
        let target_mtime = ToolsTarball::mtime()?;

        // Write out the embedded tools and scripts.
        let sealed_tools = stream::iter(
            ToolsTarball::archive()
                .entries()
                .context("Failed to list entries in tools tarball")?,
        )
        .filter_map(|tar_entry| async {
            let inner = async {
                let tar_entry = tar_entry?;

                let file_name = tar_entry
                    .path()
                    .context("Failed to find path for entry in toolbox tarball")?
                    .to_path_buf();
                match tar_entry.header().entry_type() {
                    tar::EntryType::Regular => Ok(Some(
                        SealedFile::new(tar_entry, &file_name, Some(target_mtime)).await?,
                    )),
                    // Disregard link, directories, etc
                    _ => Ok(None),
                }
            }
            .await
            .transpose();
            inner
        })
        .chain(
            stream::iter([
                ("bottlerocket-variant", BOTTLEROCKET_VARIANT),
                ("buildsys", BUILDSYS),
                ("pipesys", PIPESYS),
                ("pubsys", PUBSYS),
                ("pubsys-setup", PUBSYS_SETUP),
                ("testsys", TESTSYS),
                ("tuftool", TUFTOOL),
                ("unplug", UNPLUG),
            ])
            .then(|(name, data)| async move {
                SealedFile::new(std::io::Cursor::new(data), name, Some(target_mtime)).await
            }),
        )
        .try_collect::<Vec<_>>()
        .await?;

        let sealed_tools = sealed_tools
            .into_iter()
            .map(|sealed_file| (sealed_file.target_name.clone(), sealed_file))
            .collect();
        Ok(Tools { sealed_tools })
    }

    pub(crate) async fn with_symlinks<P: AsRef<Path>>(
        self,
        tools_dir: P,
    ) -> Result<Tools<LinkedSealedFile>> {
        let Tools { sealed_tools } = self;

        let tools_dir = tools_dir.as_ref();
        debug!("Installing tools to '{}'", tools_dir.display());
        fs::remove_dir_all(&tools_dir)
            .await
            .context("Unable to remove existing tools directory")?;
        fs::create_dir_all(&tools_dir).await.with_context(|| {
            format!("Unable to create tools directory '{}'", tools_dir.display())
        })?;

        let mut linked_tools = HashMap::with_capacity(sealed_tools.len());
        for (target_name, sealed_file) in sealed_tools.into_iter() {
            let linked = sealed_file.into_linked_sealed_file(tools_dir).await?;
            linked_tools.insert(target_name, linked);
        }

        // Finally, set the mtime on the tools directory to match the mtime of installed tools
        if let Some(tool) = linked_tools.values().next() {
            let desired_mtime = get_mtime(tool.sealed_file_path()).await?;
            set_file_mtime(tools_dir, desired_mtime).with_context(|| {
                format!(
                    "Unable to set mtime for tools dir '{}'",
                    tools_dir.display()
                )
            })?;
        }

        Ok(Tools {
            sealed_tools: linked_tools,
        })
    }
}

impl<T: SealedTool> Tools<T> {
    pub(crate) fn tool<P: AsRef<Path>>(&self, path: P) -> Option<&T> {
        self.sealed_tools.get(path.as_ref())
    }

    pub(crate) fn sealed_tools(&self) -> impl Iterator<Item = &T> {
        self.sealed_tools.values()
    }
}

pub(crate) trait SealedTool {
    fn target_name(&self) -> &Path;
    fn sealed_file_path(&self) -> &Path;
}

#[derive(Debug)]
pub(crate) struct SealedFile {
    target_name: PathBuf,
    sealed_file_path: PathBuf,
    // We hold `_sealed_file` to ensure the anonymous file remains open
    _sealed_file: File,
}

impl SealedTool for SealedFile {
    fn target_name(&self) -> &Path {
        &self.target_name
    }

    fn sealed_file_path(&self) -> &Path {
        &self.sealed_file_path
    }
}

impl SealedFile {
    pub(crate) async fn new<T, P>(
        mut source: T,
        target_name: P,
        mtime: Option<FileTime>,
    ) -> Result<Self>
    where
        T: Read,
        P: AsRef<Path>,
    {
        let target_name = target_name.as_ref().to_owned();

        let sealed = SealOptions::new()
            .close_on_exec(false)
            .executable(true)
            .copy_and_seal(&mut source)
            .context("Unable to seal file")?;

        let sealed_file = if mtime.is_some() {
            let rt = Handle::current();
            rt.spawn_blocking(move || -> Result<File> {
                set_file_handle_times(&sealed, None, mtime)
                    .context("Unable to set mtime for sealed file")?;
                Ok(sealed)
            })
            .await
            .context("Unable to run and join async task for reading handle time")??
        } else {
            sealed
        };

        let pid = std::process::id();
        let fd = sealed_file.as_raw_fd();
        let sealed_file_path = PathBuf::from(format!("/proc/{pid}/fd/{fd}"));

        Ok(Self {
            target_name,
            _sealed_file: sealed_file,
            sealed_file_path,
        })
    }

    pub(crate) async fn into_linked_sealed_file<P: AsRef<Path>>(
        self,
        target_dir: P,
    ) -> Result<LinkedSealedFile> {
        let target_dir = target_dir.as_ref();
        let symlink_path = target_dir.join(&self.target_name);
        let parent_dir = symlink_path.parent().with_context(|| {
            format!("Could not find parent dir for '{}'", symlink_path.display())
        })?;
        fs::create_dir_all(&parent_dir)
            .await
            .with_context(|| format!("Failed to create {}", parent_dir.display()))?;

        // Set the mtime for any directories we create
        let desired_mtime = get_mtime(self.sealed_file_path()).await?;
        for ancestor in symlink_path.ancestors().skip(1) {
            if ancestor.starts_with(target_dir) && ancestor != target_dir {
                set_file_mtime(ancestor, desired_mtime)
                    .with_context(|| format!("Failed to set mtime on '{}'", ancestor.display()))?;
            }
        }

        fs::symlink(&self.sealed_file_path, &symlink_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to create symlink '{}' -> '{}'",
                    symlink_path.display(),
                    self.sealed_file_path.display()
                )
            })?;

        Ok(LinkedSealedFile {
            symlink_path,
            sealed_file: self,
        })
    }
}

async fn get_mtime<P: AsRef<Path>>(path: P) -> Result<FileTime> {
    let file_meta = fs::metadata(&path)
        .await
        .with_context(|| format!("Uanble to get metadata for '{}'", path.as_ref().display()))?;
    Ok(FileTime::from_last_modification_time(&file_meta))
}

#[derive(Debug)]
pub(crate) struct LinkedSealedFile {
    symlink_path: PathBuf,
    sealed_file: SealedFile,
}

impl SealedTool for LinkedSealedFile {
    fn target_name(&self) -> &Path {
        self.sealed_file.target_name()
    }

    fn sealed_file_path(&self) -> &Path {
        self.sealed_file.sealed_file_path()
    }
}

impl Drop for LinkedSealedFile {
    fn drop(&mut self) {
        debug!("Removing {}", self.symlink_path.display());
        if let Err(e) = std::fs::remove_file(&self.symlink_path) {
            error!("Failed to remove {}: {}", self.symlink_path.display(), e);
        }
    }
}

struct ToolsTarball;

impl ToolsTarball {
    fn archive() -> Archive<impl Read> {
        Archive::new(ZlibDecoder::new(TAR_GZ_DATA))
    }

    fn mtime() -> Result<FileTime> {
        let mtime = Self::archive()
            .entries()
            .context("Failed to list entries in tools tarball")?
            .map(|e| e.context("Failed to parse entry in tools tarball"))
            .next()
            .context("No entries present in tools tarball")??
            .header()
            .mtime()
            .context("Failed to get mtime for entry in tools tarball")?;
        Ok(FileTime::from_unix_time(mtime as i64, 0))
    }
}

#[tokio::test]
async fn test_install_tools() {
    let toolsdir = tempfile::TempDir::new().unwrap();
    let tools = Tools::install().await.unwrap();

    // Assert that the expected files exist in the tools directory.

    // Check that non-binary files were copied.
    let expected_non_binaries = [
        "Makefile.toml",
        "build.Dockerfile",
        "build.Dockerfile.dockerignore",
        "docker-go",
        "img2img",
        "imghelper",
        "metadata.spec",
        "partyplanner",
        "rpm2img",
        "rpm2kit",
        "rpm2kmodkit",
        "rpm2migrations",
    ];

    // Check that binaries were copied.
    let expected_binaries = [
        "bottlerocket-variant",
        "buildsys",
        "pipesys",
        "pubsys",
        "pubsys-setup",
        "testsys",
        "tuftool",
        "unplug",
    ];

    let expected_tools = expected_non_binaries.iter().chain(expected_binaries.iter());
    for tool in expected_tools {
        let installed_tool = tools
            .tool(tool)
            .expect(format!("Did not find expected tool: {tool}").as_str());

        assert!(fs::metadata(&installed_tool.sealed_file_path())
            .await
            .unwrap()
            .is_file());
    }

    // Check that the mtimes match.
    let dockerfile = tools.tool("build.Dockerfile").unwrap();
    let dockerfile_metadata = fs::metadata(&dockerfile.sealed_file_path()).await.unwrap();

    let buildsys = tools.tool("buildsys").unwrap();
    let buildsys_metadata = fs::metadata(&buildsys.sealed_file_path()).await.unwrap();
    let dockerfile_mtime = FileTime::from_last_modification_time(&dockerfile_metadata);
    let buildsys_mtime = FileTime::from_last_modification_time(&buildsys_metadata);

    assert_eq!(dockerfile_mtime, buildsys_mtime);

    let _installed_links = tools.with_symlinks(toolsdir.path()).await.unwrap();

    let expected_tools = expected_non_binaries.iter().chain(expected_binaries.iter());
    for tool in expected_tools {
        assert!(toolsdir.path().join(tool).is_symlink());
    }
}
