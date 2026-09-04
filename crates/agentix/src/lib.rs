//! Agentix configuration and runtime assembly.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    pub channel: ChannelConfig,
    pub agent: AgentConfig,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_server_endpoint")]
    pub endpoint: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            endpoint: default_server_endpoint(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelConfig {
    pub kind: ImChannel,
    pub telegram: Option<TelegramConfig>,
    pub feishu: Option<FeishuConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImChannel {
    Telegram,
    Feishu,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AgentConfig {
    Codex {
        #[serde(default = "default_codex_endpoint")]
        endpoint: String,
        #[serde(default = "default_codex_command")]
        command: PathBuf,
        #[serde(default = "default_rmux_directory", alias = "multiplexer_directory")]
        rmux_directory: PathBuf,
    },
    Pi {
        #[serde(default = "default_pi_command")]
        command: PathBuf,
        session_dir: PathBuf,
    },
    OhMyPi {
        #[serde(default = "default_omp_command")]
        command: PathBuf,
        session_dir: PathBuf,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    #[serde(default = "default_telegram_token_env")]
    pub token_env: String,
    #[serde(default)]
    pub owner_user_ids: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeishuConfig {
    #[serde(default = "default_feishu_app_id_env")]
    pub app_id_env: String,
    #[serde(default = "default_feishu_app_secret_env")]
    pub app_secret_env: String,
    #[serde(default)]
    pub owner_open_ids: Vec<String>,
}

impl Config {
    pub fn from_toml(input: &str) -> Result<Self> {
        let mut config: Self = toml::from_str(input).context("configuration TOML is invalid")?;
        config.expand_home_paths()?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let input = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read configuration {}", path.display()))?;
        Self::from_toml(&input)
    }

    pub fn validate(&self) -> Result<()> {
        match self.channel.kind {
            ImChannel::Telegram => {
                self.channel.telegram.as_ref().context(
                    "selected telegram channel requires [channel.telegram] configuration",
                )?;
            }
            ImChannel::Feishu => {
                self.channel
                    .feishu
                    .as_ref()
                    .context("selected feishu channel requires [channel.feishu] configuration")?;
            }
        }
        if self.storage.path.as_os_str().is_empty() {
            bail!("storage.path must not be empty");
        }
        if self.server.endpoint.is_empty() {
            bail!("server.endpoint must not be empty");
        }
        if self.logging.level.trim().is_empty() {
            bail!("logging.level must not be empty");
        }
        if self.logging.file.enabled && self.logging.file.max_files == 0 {
            bail!("logging.file.max_files must be greater than zero");
        }
        if self.logging.file.enabled && self.logging.file.path.file_name().is_none() {
            bail!("logging.file.path must include a file name");
        }
        Ok(())
    }

    fn expand_home_paths(&mut self) -> Result<()> {
        let home = dirs::home_dir();
        self.storage.path = expand_home_path(&self.storage.path, home.as_deref())?;
        self.logging.file.path = expand_home_path(&self.logging.file.path, home.as_deref())?;
        self.server.endpoint =
            expand_home_in_unix_endpoint(&self.server.endpoint, home.as_deref())?;

        match &mut self.agent {
            AgentConfig::Codex {
                endpoint,
                command,
                rmux_directory,
            } => {
                *command = expand_home_path(command, home.as_deref())?;
                *rmux_directory = expand_home_path(rmux_directory, home.as_deref())?;
                *endpoint = expand_home_in_unix_endpoint(endpoint, home.as_deref())?;
            }
            AgentConfig::Pi {
                command,
                session_dir,
            }
            | AgentConfig::OhMyPi {
                command,
                session_dir,
            } => {
                *command = expand_home_path(command, home.as_deref())?;
                *session_dir = expand_home_path(session_dir, home.as_deref())?;
            }
        }

        Ok(())
    }

    pub fn telegram_token_with<F>(&self, mut read: F) -> Result<Option<String>>
    where
        F: FnMut(&str) -> Option<String>,
    {
        if self.channel.kind != ImChannel::Telegram {
            return Ok(None);
        }
        self.channel
            .telegram
            .as_ref()
            .map(|channel| required_env(&channel.token_env, &mut read))
            .transpose()
    }

    pub fn feishu_credentials_with<F>(&self, mut read: F) -> Result<Option<(String, String)>>
    where
        F: FnMut(&str) -> Option<String>,
    {
        if self.channel.kind != ImChannel::Feishu {
            return Ok(None);
        }
        self.channel
            .feishu
            .as_ref()
            .map(|channel| {
                Ok((
                    required_env(&channel.app_id_env, &mut read)?,
                    required_env(&channel.app_secret_env, &mut read)?,
                ))
            })
            .transpose()
    }
}

pub fn add_feishu_owner(path: &Path, owner_open_id: &str) -> Result<()> {
    if owner_open_id.is_empty() {
        bail!("Feishu owner open_id must not be empty");
    }
    let mut document = read_config_document(path)?;
    let feishu = feishu_table_mut(&mut document)?;
    if !feishu.contains_key("owner_open_ids") {
        feishu.insert("owner_open_ids", Item::Value(Value::Array(Array::new())));
    }
    let owners = feishu
        .get_mut("owner_open_ids")
        .and_then(Item::as_array_mut)
        .context("channel.feishu.owner_open_ids must be an array")?;
    if !owners
        .iter()
        .any(|owner| owner.as_str() == Some(owner_open_id))
    {
        owners.push(owner_open_id);
    }
    persist_config_document(path, &document)?;
    Ok(())
}

pub fn add_telegram_owner(path: &Path, owner_user_id: u64) -> Result<()> {
    if owner_user_id == 0 {
        bail!("Telegram owner user ID must not be zero");
    }
    let owner_user_id =
        i64::try_from(owner_user_id).context("Telegram owner user ID is too large")?;
    let mut document = read_config_document(path)?;
    let telegram = channel_table_mut(&mut document, "telegram")?;
    if !telegram.contains_key("owner_user_ids") {
        telegram.insert("owner_user_ids", Item::Value(Value::Array(Array::new())));
    }
    let owners = telegram
        .get_mut("owner_user_ids")
        .and_then(Item::as_array_mut)
        .context("channel.telegram.owner_user_ids must be an array")?;
    if !owners
        .iter()
        .any(|owner| owner.as_integer() == Some(owner_user_id))
    {
        owners.push(owner_user_id);
    }
    persist_config_document(path, &document)?;
    Ok(())
}

fn read_config_document(path: &Path) -> Result<DocumentMut> {
    let input = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read configuration {}", path.display()))?;
    input
        .parse::<DocumentMut>()
        .context("configuration TOML is invalid")
}

fn feishu_table_mut(document: &mut DocumentMut) -> Result<&mut Table> {
    channel_table_mut(document, "feishu")
}

fn channel_table_mut<'a>(
    document: &'a mut DocumentMut,
    channel_name: &str,
) -> Result<&'a mut Table> {
    document
        .as_table_mut()
        .get_mut("channel")
        .and_then(Item::as_table_mut)
        .and_then(|channel| channel.get_mut(channel_name))
        .and_then(Item::as_table_mut)
        .with_context(|| format!("channel.{channel_name} must be a table"))
}

fn persist_config_document(path: &Path, document: &DocumentMut) -> Result<()> {
    let parent = path.parent().with_context(|| {
        format!(
            "configuration path has no parent directory: {}",
            path.display()
        )
    })?;
    let permissions = std::fs::metadata(path)?.permissions();
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create a temporary file in {}", parent.display()))?;
    temporary.as_file().set_permissions(permissions)?;
    temporary.write_all(document.to_string().as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn expand_home_path(path: &Path, home: Option<&Path>) -> Result<PathBuf> {
    let Ok(relative) = path.strip_prefix("~") else {
        return Ok(path.to_owned());
    };
    let home = home.context("cannot expand ~ because the home directory is unavailable")?;
    Ok(home.join(relative))
}

fn expand_home_in_unix_endpoint(endpoint: &str, home: Option<&Path>) -> Result<String> {
    let Some(path) = endpoint.strip_prefix("unix://") else {
        return Ok(endpoint.to_owned());
    };
    let expanded = expand_home_path(Path::new(path), home)?;
    if expanded == Path::new(path) {
        return Ok(endpoint.to_owned());
    }
    let expanded = expanded
        .to_str()
        .context("expanded Codex endpoint path is not valid UTF-8")?;
    Ok(format!("unix://{expanded}"))
}

fn required_env<F>(name: &str, read: &mut F) -> Result<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let value = read(name).filter(|value| !value.is_empty());
    value.with_context(|| format!("required environment variable {name} is missing or empty"))
}

#[cfg(unix)]
fn default_server_endpoint() -> String {
    "unix://~/.local/share/agentix/control.sock".into()
}

#[cfg(windows)]
fn default_server_endpoint() -> String {
    "tcp://127.0.0.1:32198".into()
}

fn default_codex_endpoint() -> String {
    "unix://".into()
}

fn default_codex_command() -> PathBuf {
    "codex".into()
}

fn default_rmux_directory() -> PathBuf {
    "~".into()
}

fn default_pi_command() -> PathBuf {
    "pi".into()
}

fn default_omp_command() -> PathBuf {
    "omp".into()
}

fn default_telegram_token_env() -> String {
    "AGENTIX_TELEGRAM_TOKEN".into()
}

fn default_feishu_app_id_env() -> String {
    "AGENTIX_FEISHU_APP_ID".into()
}

fn default_feishu_app_secret_env() -> String {
    "AGENTIX_FEISHU_APP_SECRET".into()
}

fn default_log_level() -> String {
    "info".into()
}

fn default_log_file_path() -> PathBuf {
    "~/.local/state/agentix/agentix.log".into()
}

const fn default_max_log_files() -> usize {
    7
}
