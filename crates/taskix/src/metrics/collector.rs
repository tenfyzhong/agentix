//! Resolve the database shared by the host Decision metrics collector and Taskix reports.
use crate::decision::nonblank;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub(crate) fn database_path() -> Result<PathBuf> {
    if let Some(path) = nonblank(std::env::var("TASKIX_DECISION_METRICS_DB").ok())
        .or_else(|| nonblank(std::env::var("TASKIX_JEV_METRICS_DB").ok()))
    {
        return Ok(PathBuf::from(path));
    }
    let root = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map_or_else(
            || dirs::home_dir().map(|home| home.join(".local/state")),
            Some,
        )
        .context("Cannot resolve decision metrics directory")?;
    Ok(root.join("taskix/decision-metrics.sqlite"))
}
