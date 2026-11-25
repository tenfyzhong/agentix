use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use serde_json::json;

use crate::{Project, Service, WriteOptions};

/// A command-local directory identity, reusable across refreshed database reads.
/// Discovery captures the origin remote once and never registers a Project.
#[derive(Debug)]
pub struct ProjectDirectory {
    root: PathBuf,
    git: bool,
    remote: Option<String>,
}

impl ProjectDirectory {
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

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
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["remote", "get-url", "origin"])
                .output()?;
            let remote = output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .filter(|v| !v.is_empty());
            Ok(Self {
                root,
                git: true,
                remote,
            })
        } else {
            Ok(Self {
                root,
                git: false,
                remote: None,
            })
        }
    }
}

/// Discover the common repository root and its origin remote.
pub fn git_identity(root: &Path) -> Result<(PathBuf, Option<String>)> {
    let directory = ProjectDirectory::discover(root)?;
    Ok((directory.root, directory.remote))
}

impl Service {
    /// Resolve remote first and refresh a moved repository root.
    pub async fn project_for_directory(
        &self,
        directory: &ProjectDirectory,
    ) -> Result<Option<Project>> {
        if let Some(remote) = &directory.remote {
            if let Some(project) = self.store().project_by_remote(remote).await? {
                if project.root != directory.root.to_string_lossy() {
                    let result = self.execute(json!({"command":"project.register", "root":directory.root, "remote":remote, "name":project.name}), WriteOptions::default()).await?;
                    return Ok(Some(serde_json::from_value(result.result)?));
                }
                return Ok(Some(project));
            }
            // A registered path with a different origin is a different repository.
            if let Some(project) = self
                .store()
                .project_by_root(&directory.root.to_string_lossy())
                .await?
                && project.remote.is_none()
            {
                let outcome=self.execute(json!({"command":"project.register","root":directory.root,"remote":remote,"name":project.name}),WriteOptions::default()).await?;
                return Ok(Some(serde_json::from_value(outcome.result)?));
            }
            return Ok(None);
        }
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
        self.register_directory_project(directory).await.map(Some)
    }

    async fn register_directory_project(&self, directory: &ProjectDirectory) -> Result<Project> {
        let name = directory.root.file_name().map_or_else(
            || directory.root.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let outcome = self
            .execute(
                json!({"command":"project.register","name":name,"root":directory.root,"remote":directory.remote}),
                WriteOptions::default(),
            )
            .await?;
        ensure!(
            outcome.projection_pending.is_none(),
            "Project synchronization pending: {:?}",
            outcome.projection_pending
        );
        Ok(serde_json::from_value(outcome.result)?)
    }

    /// Prompt routing can register a Git project before classification.
    pub(crate) async fn project_for_prompt(&self, cwd: &Path) -> Result<Option<Project>> {
        if !cwd.is_dir() {
            return Ok(None);
        }
        let directory = ProjectDirectory::discover(cwd)?;
        if let Some(project) = self.project_for_directory(&directory).await? {
            return Ok(Some(project));
        }
        if directory.git {
            return self.register_directory_project(&directory).await.map(Some);
        }
        Ok(None)
    }

    /// IM policy: resolve the current directory; use history only without a usable cwd.
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
