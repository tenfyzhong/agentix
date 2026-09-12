//! Agentix configuration and runtime assembly.

mod config_file;
mod engine_runtime;
mod notification_runtime;
pub use engine_runtime::{run_engine_loop, run_engine_loop_with_config, shutdown_engine};
mod network;

pub use network::NetworkConfig;

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "config_file::ConfigFile")]
pub struct Config {
    pub multiplexer: MultiplexerConfig,
    #[serde(default)]
    pub slack_cli_path: Option<PathBuf>,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub output: agentix_core::OutputConfig,
    pub channel: ChannelConfig,
    #[serde(default)]
    pub agent: Option<AgentConfig>,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    pub storage: StorageConfig,
    #[serde(default)]
    pub task_board: Option<TaskBoardConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct MultiplexerConfig {
    pub kind: agentix_core::MultiplexerKind,
    pub working_dir: PathBuf,
}
impl Default for MultiplexerConfig {
    fn default() -> Self {
        Self {
            kind: agentix_core::MultiplexerKind::default(),
            working_dir: PathBuf::from("~"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBoardConfig {
    #[serde(default)]
    pub enable: bool,
    pub config: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationConfig {
    pub background_turns: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            background_turns: true,
        }
    }
}

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
    pub slack: Option<SlackConfig>,
}

pub use agentix_core::ChannelKind as ImChannel;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AgentConfig {
    Claude {
        #[serde(default = "default_claude_command")]
        command: PathBuf,
        session_dir: PathBuf,
    },
    Codex {
        #[serde(default = "default_codex_endpoint")]
        endpoint: String,
        #[serde(default = "default_codex_proxy_endpoint")]
        proxy_endpoint: String,
        #[serde(default)]
        proxy: agentix_codex::ProxyOptions,
        #[serde(default = "default_codex_command")]
        command: PathBuf,
    },
    Pi {
        #[serde(default = "default_pi_command")]
        command: PathBuf,
        session_dir: PathBuf,
        #[serde(default)]
        bridge_extension: Option<PathBuf>,
    },
    #[serde(alias = "omp")]
    OhMyPi {
        #[serde(default = "default_omp_command")]
        command: PathBuf,
        session_dir: PathBuf,
        #[serde(default)]
        bridge_extension: Option<PathBuf>,
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
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub owner_user_ids: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeishuConfig {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    #[serde(default)]
    pub owner_open_ids: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlackConfig {
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub command_prefix: String,
    #[serde(default)]
    pub command_suffix: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub app_token: String,
    #[serde(default)]
    pub owner_user_ids: Vec<String>,
}

impl SlackConfig {
    fn validate_commands(&self) -> Result<()> {
        let affixes =
            agentix_slack::CommandAffixes::new(&self.command_prefix, &self.command_suffix)?;
        for command in agentix_core::command_menu(true).commands {
            affixes.encode(&command.name)?;
        }
        for name in [
            "agentix",
            "claim",
            "dashboard",
            "board",
            "jobs",
            "inboxes",
            "inbox",
        ] {
            affixes.encode(name)?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for SlackConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SlackConfig")
            .field("app_id", &self.app_id)
            .field("command_prefix", &self.command_prefix)
            .field("command_suffix", &self.command_suffix)
            .field("bot_token", &"[redacted]")
            .field("app_token", &"[redacted]")
            .field("owner_user_ids", &self.owner_user_ids)
            .finish()
    }
}

impl AgentConfig {
    #[must_use]
    pub const fn kind(&self) -> agentix_core::AgentKind {
        match self {
            Self::Claude { .. } => agentix_core::AgentKind::Claude,
            Self::Codex { .. } => agentix_core::AgentKind::Codex,
            Self::Pi { .. } => agentix_core::AgentKind::Pi,
            Self::OhMyPi { .. } => agentix_core::AgentKind::Omp,
        }
    }
}

impl Config {
    pub fn apply_codex_proxy_options(
        &mut self,
        options: &agentix_codex::ProxyOptions,
    ) -> Result<()> {
        if options == &agentix_codex::ProxyOptions::default() {
            return Ok(());
        }
        let mut found = false;
        for agent in self.agent.iter_mut().chain(&mut self.agents) {
            if let AgentConfig::Codex { proxy, .. } = agent {
                proxy.overlay(options);
                found = true;
            }
        }
        anyhow::ensure!(
            found,
            "WebSocket proxy flags require a configured Codex backend"
        );
        self.expand_home_paths()
    }

    #[must_use]
    pub fn selected_agents(&self) -> Vec<&AgentConfig> {
        self.agent.iter().chain(self.agents.iter()).collect()
    }

    #[must_use]
    pub fn enabled_task_board(&self) -> Option<&TaskBoardConfig> {
        self.task_board
            .as_ref()
            .filter(|task_board| task_board.enable)
    }

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
        self.network.validate()?;
        if self
            .slack_cli_path
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            bail!("slack_cli_path must be an absolute executable path");
        }
        if self.agent.is_some() && !self.agents.is_empty() {
            bail!("use either [agent] or [[agents]], not both");
        }
        let selected = self.selected_agents();
        if selected.is_empty() {
            bail!("configure at least one agent backend");
        }
        let mut kinds = std::collections::HashSet::new();
        for agent in selected {
            if !kinds.insert(agent.kind()) {
                bail!("duplicate agent backend");
            }
        }

        match self.channel.kind {
            ImChannel::Slack => {
                let slack = self
                    .channel
                    .slack
                    .as_ref()
                    .context("selected slack channel requires [channel.slack] configuration")?;
                slack.validate_commands()?;
                if slack.app_id.as_ref().is_some_and(|id| {
                    !id.starts_with('A')
                        || id.len() < 2
                        || !id.bytes().all(|c| c.is_ascii_alphanumeric())
                }) {
                    bail!("channel.slack.app_id must be a Slack app ID starting with A");
                }
                if slack.bot_token.trim().is_empty() {
                    bail!("channel.slack.bot_token must not be missing or blank");
                }
                if slack.app_token.trim().is_empty() {
                    bail!("channel.slack.app_token must not be missing or blank");
                }
                if slack
                    .owner_user_ids
                    .iter()
                    .any(|owner| owner.trim().is_empty())
                {
                    bail!("channel.slack.owner_user_ids must not contain blank IDs");
                }
            }
            ImChannel::Telegram => {
                let telegram = self.channel.telegram.as_ref().context(
                    "selected telegram channel requires [channel.telegram] configuration",
                )?;
                if telegram.token.trim().is_empty() {
                    bail!("channel.telegram.token must not be missing or blank");
                }
            }
            ImChannel::Feishu => {
                let feishu =
                    self.channel.feishu.as_ref().context(
                        "selected feishu channel requires [channel.feishu] configuration",
                    )?;
                if feishu.app_id.trim().is_empty() {
                    bail!("channel.feishu.app_id must not be missing or blank");
                }
                if feishu.app_secret.trim().is_empty() {
                    bail!("channel.feishu.app_secret must not be missing or blank");
                }
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
        self.multiplexer.working_dir =
            expand_home_path(&self.multiplexer.working_dir, home.as_deref())?;
        if let Some(task_board) = &mut self.task_board {
            task_board.config = expand_home_path(&task_board.config, home.as_deref())?;
        }
        self.storage.path = expand_home_path(&self.storage.path, home.as_deref())?;
        self.logging.file.path = expand_home_path(&self.logging.file.path, home.as_deref())?;
        self.server.endpoint =
            expand_home_in_unix_endpoint(&self.server.endpoint, home.as_deref())?;

        for agent in self.agent.iter_mut().chain(self.agents.iter_mut()) {
            match agent {
                AgentConfig::Claude {
                    command,
                    session_dir,
                } => {
                    *command = expand_home_path(command, home.as_deref())?;
                    *session_dir = expand_home_path(session_dir, home.as_deref())?;
                }
                AgentConfig::Codex {
                    endpoint,
                    proxy_endpoint,
                    proxy,
                    command,
                } => {
                    *command = expand_home_path(command, home.as_deref())?;
                    *endpoint = expand_home_in_unix_endpoint(endpoint, home.as_deref())?;
                    *proxy_endpoint =
                        expand_home_in_unix_endpoint(proxy_endpoint, home.as_deref())?;
                    for path in [&mut proxy.ws_token_file, &mut proxy.ws_shared_secret_file]
                        .into_iter()
                        .flatten()
                    {
                        *path = expand_home_path(path, home.as_deref())?;
                    }
                }
                AgentConfig::Pi {
                    command,
                    session_dir,
                    bridge_extension,
                }
                | AgentConfig::OhMyPi {
                    command,
                    session_dir,
                    bridge_extension,
                } => {
                    *command = expand_home_path(command, home.as_deref())?;
                    *session_dir = expand_home_path(session_dir, home.as_deref())?;
                    if let Some(path) = bridge_extension {
                        *path = expand_home_path(path, home.as_deref())?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn add_slack_owner(path: &Path, owner_user_id: &str) -> Result<()> {
    if owner_user_id.trim().is_empty() {
        bail!("Slack owner user ID must not be blank");
    }
    let mut document = read_config_document(path)?;
    let slack = channel_table_mut(&mut document, "slack")?;
    if !slack.contains_key("owner_user_ids") {
        slack.insert("owner_user_ids", Item::Value(Value::Array(Array::new())));
    }
    let owners = slack
        .get_mut("owner_user_ids")
        .and_then(Item::as_array_mut)
        .context("channel.slack.owner_user_ids must be an array")?;
    if !owners
        .iter()
        .any(|owner| owner.as_str() == Some(owner_user_id))
    {
        owners.push(owner_user_id);
    }
    persist_config_document(path, &document)
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

#[cfg(unix)]
fn default_server_endpoint() -> String {
    "unix://~/.local/share/agentix/control.sock".into()
}

#[cfg(windows)]
fn default_server_endpoint() -> String {
    "tcp://127.0.0.1:32198".into()
}

fn default_codex_endpoint() -> String {
    agentix_codex::CodexEndpoint::default_upstream().map_or_else(
        |_| "unix://~/.codex/app-server-control/app-server-control-upstream.sock".into(),
        |e| e.address(),
    )
}

fn default_codex_proxy_endpoint() -> String {
    "unix://".into()
}

fn default_codex_command() -> PathBuf {
    "codex".into()
}

fn default_pi_command() -> PathBuf {
    "pi".into()
}

fn default_omp_command() -> PathBuf {
    "omp".into()
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

fn default_claude_command() -> PathBuf {
    PathBuf::from("claude")
}
