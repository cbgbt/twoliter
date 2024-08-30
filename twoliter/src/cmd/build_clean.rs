use crate::cargo_make::CargoMake;
use crate::lock::Lock;
use crate::project;
use crate::tools::Tools;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) struct BuildClean {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent.
    #[clap(long = "project-path")]
    project_path: Option<PathBuf>,
}

impl BuildClean {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let lock = Lock::load(&project).await?;

        let tools_tempdir = tempfile::TempDir::new().unwrap();
        let toolsdir = tools_tempdir.path();
        let _tools = Tools::install().await?.with_symlinks(&toolsdir).await?;

        let makefile_path = toolsdir.join("Makefile.toml");

        CargoMake::new(&lock.sdk.source)?
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec("clean")
            .await?;

        Ok(())
    }
}
