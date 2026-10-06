use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub storage: StorageConfig,
    pub documents: DocumentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    pub path: PathBuf,
}

impl StorageConfig {
    /// Load only the task source connection, without opening a document vault.
    pub fn load(path: &Path) -> Result<Self> {
        #[derive(Deserialize)]
        struct SourceConfig {
            schema_version: u32,
            storage: StorageConfig,
        }
        let path = expand_home(path)?;
        let mut config: SourceConfig = toml::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("read task config {}", path.display()))?,
        )?;
        ensure!(
            config.schema_version == 1,
            "unsupported task config schema_version"
        );
        config.storage.path = expand_home(&config.storage.path)?;
        ensure!(
            config.storage.path.is_absolute() && config.storage.path.file_name().is_some(),
            "storage.path must be an absolute file path"
        );
        Ok(config.storage)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentConfig {
    pub root: PathBuf,
    pub directory: PathBuf,
    #[serde(default = "default_archive_directory")]
    pub archive_directory: PathBuf,
}

fn default_archive_directory() -> PathBuf {
    "Archived Projects".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let path = expand_home(path)?;
        let mut value: toml::Value = toml::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("read task config {}", path.display()))?,
        )?;
        // Memory owns its capability configuration and validates it at its entrypoint.
        // In particular, unavailable providers must not prevent board operations.
        if let Some(table) = value.as_table_mut() {
            table.remove("memory");
            // Logging belongs to the resident service, not board operations.
            table.remove("logging");
        }
        // Older releases stored a template locale and output format here.
        // Tolerate obsolete keys without exposing them; all output is Obsidian.
        if let Some(documents) = value
            .get_mut("documents")
            .and_then(toml::Value::as_table_mut)
        {
            documents.remove("language");
            documents.remove("format");
        }
        let mut config: Self = value.try_into()?;
        config.storage.path = expand_home(&config.storage.path)?;
        config.documents.root = expand_home(&config.documents.root)?;
        config.validate()?;
        Ok(config)
    }
    #[must_use]
    pub fn output_dir(&self) -> PathBuf {
        self.documents.root.join(&self.documents.directory)
    }
    /// Absolute archive destination, independent of the active document output.
    #[must_use]
    pub fn archive_dir(&self) -> PathBuf {
        self.documents.root.join(&self.documents.archive_directory)
    }

    /// Convert a registered document path into an Obsidian vault-relative path.
    #[must_use]
    pub fn vault_relative_path(&self, path: &Path) -> PathBuf {
        path.strip_prefix("Archived Projects").map_or_else(
            |_| self.documents.directory.join(path),
            |relative| self.documents.archive_directory.join(relative),
        )
    }

    /// Resolve a logical relative document path against the current configuration.
    pub fn document_path(&self, relative: &Path) -> Result<PathBuf> {
        ensure!(
            !relative.is_absolute()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "invalid relative document path"
        );
        let boundary = if relative.starts_with("Archived Projects") {
            self.archive_dir()
        } else {
            self.output_dir()
        };
        let path = self.documents.root.join(self.vault_relative_path(relative));
        ensure!(
            resolved_path(&path)?.starts_with(resolved_path(&boundary)?),
            "document path escapes its root"
        );
        Ok(path)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported task config schema_version"
        );
        ensure!(
            self.documents.root.is_absolute() && self.documents.root.is_dir(),
            "documents.root must be an existing absolute directory"
        );
        ensure!(
            !self.documents.directory.as_os_str().is_empty()
                && self
                    .documents
                    .directory
                    .components()
                    .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "documents.directory must be a relative path without traversal"
        );
        let archive = &self.documents.archive_directory;
        ensure!(
            !archive.as_os_str().is_empty()
                && archive
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
                && !archive.starts_with("Projects"),
            "documents.archive_directory must be a relative directory outside Projects without traversal"
        );
        ensure!(
            resolved_path(&self.archive_dir())?.starts_with(self.documents.root.canonicalize()?),
            "archive directory escapes document root"
        );
        let archive = resolved_path(&self.archive_dir())?;
        let projects = resolved_path(&self.output_dir().join("Projects"))?;
        ensure!(
            !archive.starts_with(&projects) && !projects.starts_with(&archive),
            "archive directory overlaps active Projects"
        );
        ensure!(
            !resolved_path(&self.storage.path)?.starts_with(&archive),
            "task database must be outside the archive directory"
        );
        ensure!(
            self.storage.path.is_absolute() && self.storage.path.file_name().is_some(),
            "storage.path must be an absolute file path"
        );
        ensure!(
            !resolved_path(&self.storage.path)?.starts_with(resolved_path(&self.output_dir())?),
            "task database must be outside the document output directory"
        );
        ensure!(
            resolved_path(&self.output_dir())?.starts_with(self.documents.root.canonicalize()?),
            "document output escapes its root"
        );
        ensure!(
            self.documents.root.join(".obsidian").is_dir(),
            "Obsidian root must contain .obsidian"
        );
        Ok(())
    }
    pub fn default_path() -> Result<PathBuf> {
        expand_home(Path::new("~/.config/taskix/config.toml"))
    }
}

pub fn expand_home(path: &Path) -> Result<PathBuf> {
    if let Ok(relative) = path.strip_prefix("~") {
        Ok(dirs::home_dir()
            .context("home directory unavailable")?
            .join(relative))
    } else {
        Ok(path.to_owned())
    }
}

pub(crate) fn resolved_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path.parent().context("path has no parent")?;
    Ok(resolved_path(parent)?.join(path.file_name().context("path has no file name")?))
}
