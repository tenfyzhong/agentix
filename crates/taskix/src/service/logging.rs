use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder as RollingBuilder, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file: FileLoggingConfig,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: FileLoggingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileLoggingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_log_file_path")]
    pub path: PathBuf,
    #[serde(default)]
    pub rotation: LogRotation,
    #[serde(default = "default_max_log_files")]
    pub max_files: usize,
}

impl Default for FileLoggingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: default_log_file_path(),
            rotation: LogRotation::Daily,
            max_files: default_max_log_files(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogRotation {
    Never,
    Minutely,
    Hourly,
    #[default]
    Daily,
}

impl LoggingConfig {
    pub fn load(path: &Path) -> Result<Self> {
        #[derive(Deserialize)]
        struct ServiceConfig {
            #[serde(default)]
            logging: LoggingConfig,
        }
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("read task config {}", path.display()))?;
        let mut config = toml::from_str::<ServiceConfig>(&source)?.logging;
        ensure!(
            !config.level.trim().is_empty(),
            "logging.level must not be empty"
        );
        EnvFilter::try_new(&config.level).context("logging.level is not a valid tracing filter")?;
        ensure!(
            !config.file.enabled || config.file.max_files > 0,
            "logging.file.max_files must be greater than zero"
        );
        ensure!(
            !config.file.enabled || config.file.path.file_name().is_some(),
            "logging.file.path must include a file name"
        );
        config.file.path = agentix_task::expand_home(&config.file.path)?;
        Ok(config)
    }
}

fn default_log_level() -> String {
    "info".into()
}

fn default_log_file_path() -> PathBuf {
    "~/.local/state/taskix/taskix.log".into()
}

const fn default_max_log_files() -> usize {
    7
}

pub fn init(config: &LoggingConfig) -> Result<Option<WorkerGuard>> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.level))
        .context("logging.level is not a valid tracing filter")?;
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_timer(local_time_timer());

    if !config.file.enabled {
        tracing_subscriber::registry()
            .with(filter)
            .with(stderr)
            .try_init()
            .context("failed to initialize logging")?;
        return Ok(None);
    }

    let parent = config
        .file
        .path
        .parent()
        .context("logging.file.path has no parent directory")?;
    let file_name = config
        .file
        .path
        .file_name()
        .context("logging.file.path must include a file name")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create log directory {}", parent.display()))?;
    let rotation = match config.file.rotation {
        LogRotation::Never => Rotation::NEVER,
        LogRotation::Minutely => Rotation::MINUTELY,
        LogRotation::Hourly => Rotation::HOURLY,
        LogRotation::Daily => Rotation::DAILY,
    };
    let appender = RollingBuilder::new()
        .rotation(rotation)
        .filename_prefix(file_name.to_string_lossy())
        .max_log_files(config.file.max_files)
        .build(parent)
        .context("failed to initialize rolling file logging")?;
    let (file_writer, guard) = tracing_appender::non_blocking(appender);
    let file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(file_writer)
        .with_timer(local_time_timer());
    tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file)
        .try_init()
        .context("failed to initialize logging")?;
    Ok(Some(guard))
}

fn local_time_timer() -> impl tracing_subscriber::fmt::time::FormatTime {
    tracing_subscriber::fmt::time::LocalTime::rfc_3339()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_agentix_with_a_taskix_specific_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "schema_version=1\n").unwrap();
        let config = LoggingConfig::load(&path).unwrap();
        assert_eq!(config.level, "info");
        assert!(!config.file.enabled);
        assert_eq!(config.file.rotation, LogRotation::Daily);
        assert_eq!(config.file.max_files, 7);
        assert_eq!(
            config.file.path,
            dirs::home_dir()
                .unwrap()
                .join(".local/state/taskix/taskix.log")
        );
    }

    #[test]
    fn parses_filters_rotations_and_expands_home() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for (name, rotation) in [
            ("never", LogRotation::Never),
            ("minutely", LogRotation::Minutely),
            ("hourly", LogRotation::Hourly),
            ("daily", LogRotation::Daily),
        ] {
            std::fs::write(&path, format!("[logging]\nlevel='taskix=debug,agentix_memory=trace'\n[logging.file]\nenabled=true\npath='~/logs/taskix.log'\nrotation='{name}'\nmax_files=12\n")).unwrap();
            let config = LoggingConfig::load(&path).unwrap();
            assert_eq!(config.level, "taskix=debug,agentix_memory=trace");
            assert_eq!(config.file.rotation, rotation);
            assert_eq!(config.file.max_files, 12);
            assert_eq!(
                config.file.path,
                dirs::home_dir().unwrap().join("logs/taskix.log")
            );
        }
    }
}
