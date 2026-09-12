//! Decode named backend tables while retaining legacy configuration compatibility.
use super::{
    AgentConfig, ChannelConfig, Config, LoggingConfig, NetworkConfig, NotificationConfig,
    ServerConfig, StorageConfig, TaskBoardConfig,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigFile {
    #[serde(default)]
    pub(super) multiplexer: super::MultiplexerConfig,
    #[serde(default)]
    pub(super) slack_cli_path: Option<std::path::PathBuf>,
    #[serde(default)]
    pub(super) network: NetworkConfig,
    #[serde(default)]
    pub(super) server: ServerConfig,
    #[serde(default)]
    pub(super) logging: LoggingConfig,
    #[serde(default)]
    pub(super) notifications: NotificationConfig,
    #[serde(default)]
    pub(super) output: agentix_core::OutputConfig,
    pub(super) channel: ChannelConfig,
    #[serde(default)]
    pub(super) agent: Option<toml::Value>,
    #[serde(default)]
    pub(super) agents: Vec<AgentConfig>,
    pub(super) storage: StorageConfig,
    #[serde(default)]
    pub(super) task_board: Option<TaskBoardConfig>,
}

impl TryFrom<ConfigFile> for Config {
    type Error = anyhow::Error;

    fn try_from(mut file: ConfigFile) -> Result<Self> {
        let agent = match file.agent.take() {
            None => None,
            Some(value) if value.get("kind").is_some() => Some(value.try_into::<AgentConfig>()?),
            Some(toml::Value::Table(backends)) => {
                if !file.agents.is_empty() {
                    bail!("do not combine [agent.<backend>] with [[agents]]");
                }
                for (name, value) in backends {
                    if !matches!(name.as_str(), "codex" | "pi" | "omp" | "claude") {
                        bail!("unknown agent backend '{name}'; expected codex, pi, omp, or claude");
                    }
                    let toml::Value::Table(mut fields) = value else {
                        bail!("agent.{name} must be a table");
                    };
                    if fields.contains_key("kind") {
                        bail!("agent.{name} determines the backend; remove its kind field");
                    }
                    fields.insert("kind".into(), toml::Value::String(name.clone()));
                    file.agents.push(
                        toml::Value::Table(fields)
                            .try_into::<AgentConfig>()
                            .with_context(|| format!("invalid agent.{name} configuration"))?,
                    );
                }
                None
            }
            Some(_) => bail!("agent must be a table"),
        };
        Ok(Self {
            multiplexer: file.multiplexer,
            slack_cli_path: file.slack_cli_path,
            network: file.network,
            server: file.server,
            logging: file.logging,
            notifications: file.notifications,
            output: file.output,
            channel: file.channel,
            agent,
            agents: file.agents,
            storage: file.storage,
            task_board: file.task_board,
        })
    }
}
