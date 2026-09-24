use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use serde_json::json;

use crate::{Project, Service, WriteOptions};

/// A command-local directory identity, reusable across refreshed database reads.
/// Discovery never reads remotes or registers a Project.
#[derive(Debug)]
pub struct ProjectDirectory {
    root: PathBuf,
    git: bool,
}

impl ProjectDirectory {
    pub fn discover(path: &Path) -> Result<Self> {
        let root = path.canonicalize()?;
        let output = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            let common = PathBuf::from(String::from_utf8(output.stdout)?.trim()).canonicalize()?;
            let root = common
                .parent()
                .context("Git common directory has no parent")?
                .to_owned();
            Ok(Self { root, git: true })
        } else {
            Ok(Self { root, git: false })
        }
    }
}

/// Registration metadata; unlike lookup, this explicitly reads the Git remote.
pub fn git_identity(root: &Path) -> Result<(PathBuf, Option<String>)> {
    let directory = ProjectDirectory::discover(root)?;
    let remote = if directory.git {
        let output = Command::new("git")
            .arg("-C")
            .arg(&directory.root)
            .args(["remote", "get-url", "origin"])
            .output()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        None
    };
    Ok((directory.root, remote))
}

impl Service {
    /// Read exactly one registered Project, without filesystem probes or writes.
    pub async fn project_for_directory(
        &self,
        directory: &ProjectDirectory,
    ) -> Result<Option<Project>> {
        self.store()
            .project_by_root(&directory.root.to_string_lossy())
            .await
    }

    /// CLI policy: create missing non-Git Projects, but require Git registration.
    /// The transaction rechecks identity and name allocation for concurrent callers.
    pub async fn ensure_directory_project(
        &self,
        directory: &ProjectDirectory,
    ) -> Result<Option<Project>> {
        if let Some(project) = self.project_for_directory(directory).await? {
            return Ok(Some(project));
        }
        if directory.git {
            return Ok(None);
        }
        let name = directory.root.file_name().map_or_else(
            || directory.root.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let outcome = self
            .execute(
                json!({"command":"project.register","name":name,"root":directory.root}),
                WriteOptions::default(),
            )
            .await?;
        ensure!(
            outcome.projection_pending.is_none(),
            "Project synchronization pending: {:?}",
            outcome.projection_pending
        );
        Ok(Some(serde_json::from_value(outcome.result)?))
    }

    /// IM policy: directory lookup is read-only; history is used only without a usable cwd.
    pub async fn project_for_session(
        &self,
        cwd: Option<&Path>,
        session: Option<&str>,
    ) -> Result<Option<Project>> {
        if let Some(cwd) = cwd.filter(|p| p.is_dir()) {
            return self
                .project_for_directory(&ProjectDirectory::discover(cwd)?)
                .await;
        }
        let Some(session) = session else {
            return Ok(None);
        };
        self.store().session_project(session).await
    }
}
