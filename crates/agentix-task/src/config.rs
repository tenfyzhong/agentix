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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentConfig {
    pub format: DocumentFormat,
    pub root: PathBuf,
    pub directory: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentFormat {
    Obsidian,
    Markdown,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let path = expand_home(path)?;
        let mut config: Self = toml::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("read task config {}", path.display()))?,
        )?;
        config.storage.path = expand_home(&config.storage.path)?;
        config.documents.root = expand_home(&config.documents.root)?;
        config.validate()?;
        Ok(config)
    }
    #[must_use]
    pub fn output_dir(&self) -> PathBuf {
        self.documents.root.join(&self.documents.directory)
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
        if self.documents.format == DocumentFormat::Obsidian {
            ensure!(
                self.documents.root.join(".obsidian").is_dir(),
                "Obsidian root must contain .obsidian"
            );
        }
        Ok(())
    }
    pub fn default_path() -> Result<PathBuf> {
        expand_home(Path::new("~/.config/taskcli/config.toml"))
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
