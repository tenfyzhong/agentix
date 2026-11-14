//! Local directory policy for automatic intake; explicit management remains available.
use crate::{ProjectDirectory, expand_home};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrackingConfig {
    pub exclude_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackingStatus {
    pub enabled: bool,
    pub matched_rule: Option<PathBuf>,
    pub matched_path: Option<PathBuf>,
}

impl TrackingConfig {
    /// Read only policy, without opening storage or validating the document vault.
    pub fn load(path: &Path) -> Result<Self> {
        #[derive(Deserialize)]
        struct Policy {
            #[serde(default)]
            tracking: TrackingConfig,
        }
        let value: Policy = toml::from_str(&std::fs::read_to_string(expand_home(path)?)?)?;
        value.tracking.validate()?;
        Ok(value.tracking)
    }

    pub fn validate(&self) -> Result<()> {
        for rule in &self.exclude_paths {
            ensure!(
                expand_home(rule)?.is_absolute(),
                "tracking.exclude_paths must contain absolute paths or ~/ paths"
            );
        }
        Ok(())
    }

    pub fn check(&self, path: &Path) -> Result<TrackingStatus> {
        self.validate()?;
        let mut status = TrackingStatus {
            enabled: true,
            matched_rule: None,
            matched_path: None,
        };
        if self.exclude_paths.is_empty() {
            return Ok(status);
        }
        let current = normalize(path)?;
        let root = if current.is_dir() {
            Some(ProjectDirectory::discover(&current)?.root().to_owned())
        } else {
            None
        };
        for candidate in std::iter::once(&current).chain(root.as_ref()) {
            for rule in &self.exclude_paths {
                if candidate.starts_with(normalize(rule)?) {
                    status.enabled = false;
                    status.matched_rule = Some(rule.clone());
                    status.matched_path = Some(candidate.clone());
                    return Ok(status);
                }
            }
        }
        Ok(status)
    }
}

fn normalize(path: &Path) -> Result<PathBuf> {
    let path = expand_home(path)?;
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => (),
            Component::ParentDir => {
                resolved.pop();
            }
            other => {
                resolved.push(other.as_os_str());
                match resolved.canonicalize() {
                    Ok(real) => resolved = real,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error).context("resolve tracking path"),
                }
            }
        }
    }
    Ok(resolved)
}
