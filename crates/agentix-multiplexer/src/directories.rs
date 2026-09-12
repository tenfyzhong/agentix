//! Local directory access shared by workspace runtimes, independent of terminal drivers.
use std::path::{Path, PathBuf};

use agentix_domain::{WorkspaceDirectoryEntry, WorkspaceDirectoryPage};

use crate::{MultiplexerError, WorkspaceManager};

const PAGE_SIZE: usize = 6;

impl WorkspaceManager {
    pub async fn resolve_directory(
        &self,
        input: &str,
        base: &str,
    ) -> Result<String, MultiplexerError> {
        let home = self.default_directory().to_path_buf();
        let input = input.to_owned();
        let base = base.to_owned();
        tokio::task::spawn_blocking(move || resolve(&input, &base, &home))
            .await
            .map_err(|error| MultiplexerError::InvalidWorkspace(error.to_string()))?
    }

    pub async fn list_directories(
        &self,
        directory: &str,
        page: usize,
        show_hidden: bool,
    ) -> Result<WorkspaceDirectoryPage, MultiplexerError> {
        let directory = self
            .resolve_directory(directory, &self.default_directory().to_string_lossy())
            .await?;
        tokio::task::spawn_blocking(move || list(&directory, page, show_hidden))
            .await
            .map_err(|error| MultiplexerError::InvalidWorkspace(error.to_string()))?
    }
}

fn resolve(input: &str, base: &str, home: &Path) -> Result<String, MultiplexerError> {
    if input.is_empty() || input.contains(['\0', '\n', '\r']) {
        return Err(MultiplexerError::InvalidWorkspace(
            "enter a directory path".into(),
        ));
    }
    let path = if input == "~" {
        home.to_path_buf()
    } else if let Some(suffix) = input.strip_prefix("~/") {
        home.join(suffix)
    } else {
        let path = PathBuf::from(input);
        if path.is_absolute() {
            path
        } else {
            let base = Path::new(base);
            if !base.is_absolute() {
                return Err(MultiplexerError::InvalidWorkspace(
                    "relative directory base must be absolute".into(),
                ));
            }
            base.join(path)
        }
    };
    let path = path
        .canonicalize()
        .map_err(|error| invalid(&path, &error))?;
    if !path.is_dir() {
        return Err(MultiplexerError::InvalidWorkspace(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    path.into_os_string()
        .into_string()
        .map_err(|_| MultiplexerError::InvalidWorkspace("directory is not valid UTF-8".into()))
}

fn invalid(path: &Path, error: &std::io::Error) -> MultiplexerError {
    MultiplexerError::InvalidWorkspace(format!("{}: {error}", path.display()))
}

fn list(
    directory: &str,
    requested_page: usize,
    show_hidden: bool,
) -> Result<WorkspaceDirectoryPage, MultiplexerError> {
    let path = Path::new(directory);
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|error| invalid(path, &error))? {
        let entry = entry.map_err(|error| invalid(path, &error))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if (!show_hidden && name.starts_with('.')) || name.contains(['\n', '\r']) {
            continue;
        }
        // Follow directory symlinks; broken or inaccessible entries cannot be browsed.
        if entry.path().is_dir() {
            entries.push(WorkspaceDirectoryEntry {
                name,
                path: entry.path().to_string_lossy().into_owned(),
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let pages = entries.len().div_ceil(PAGE_SIZE).max(1);
    let page = requested_page.min(pages - 1);
    Ok(WorkspaceDirectoryPage {
        directory: directory.into(),
        parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
        page,
        pages,
        entries: entries
            .into_iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .collect(),
    })
}
