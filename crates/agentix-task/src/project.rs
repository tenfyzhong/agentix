use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};

use crate::{Project, Service};

/// Return the repository root shared by all worktrees, or a stable non-Git root.
pub fn git_identity(root: &Path) -> Result<(PathBuf, Option<String>)> {
    let (root, remote, _) = directory_identity(root)?;
    Ok((root, remote))
}

fn directory_identity(root: &Path) -> Result<(PathBuf, Option<String>, bool)> {
    let root = root.canonicalize()?;
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
        let remote = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["remote", "get-url", "origin"])
            .output()?;
        let remote = remote
            .status
            .success()
            .then(|| String::from_utf8_lossy(&remote.stdout).trim().to_owned());
        Ok((root, remote, true))
    } else {
        Ok((root, None, false))
    }
}

impl Service {
    /// Resolve an attached session's directory; without one, require a unique
    /// historical Project association. Never use the Agentix daemon's cwd.
    pub async fn project_for_session(
        &self,
        cwd: Option<&Path>,
        session: Option<&str>,
    ) -> Result<Option<Project>> {
        if let Some(cwd) = cwd.filter(|p| p.is_dir()) {
            let (root, _, git) = directory_identity(cwd)?;
            let projects = self.store().projects().await?;
            return Ok(projects
                .into_iter()
                .filter(|p| {
                    let registered = Path::new(&p.root)
                        .canonicalize()
                        .unwrap_or_else(|_| PathBuf::from(&p.root));
                    if git {
                        root == registered
                    } else {
                        root.starts_with(registered)
                    }
                })
                .min_by_key(|p| std::cmp::Reverse(Path::new(&p.root).components().count())));
        }
        let Some(session) = session else {
            return Ok(None);
        };
        self.store().session_project(session).await
    }
}
