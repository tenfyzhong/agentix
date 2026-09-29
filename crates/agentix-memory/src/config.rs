use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    #[default]
    Openai,
    Ollama,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelApi {
    #[default]
    Responses,
    ChatCompletions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    #[serde(default)]
    pub protocol: ProviderProtocol,
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "provider_concurrency")]
    pub max_in_flight: usize,
}
fn provider_concurrency() -> usize {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    pub provider: String,
    pub api: ModelApi,
    pub model: String,
    pub max_concurrent_loops: usize,
    pub max_extraction_loops_per_project: usize,
    pub max_steps: usize,
    pub max_tool_calls: usize,
    pub max_context_bytes: usize,
    pub max_output_tokens: usize,
    pub request_timeout_seconds: u64,
    pub task_timeout_seconds: u64,
    pub lease_seconds: u64,
    pub max_attempts: u32,
    pub repository_review_interval_seconds: u64,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            api: ModelApi::Responses,
            model: "gpt-6-astra".into(),
            max_concurrent_loops: 4,
            max_extraction_loops_per_project: 2,
            max_steps: 12,
            max_tool_calls: 24,
            max_context_bytes: 128 * 1024,
            max_output_tokens: 8192,
            request_timeout_seconds: 90,
            task_timeout_seconds: 240,
            lease_seconds: 300,
            max_attempts: 3,
            repository_review_interval_seconds: 86400,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbeddingConfig {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub dimensions: Option<usize>,
    pub query_prefix: String,
    pub document_prefix: String,
    pub request_timeout_seconds: u64,
    pub batch_size: usize,
}
impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            dimensions: None,
            query_prefix: String::new(),
            document_prefix: String::new(),
            request_timeout_seconds: 10,
            batch_size: 16,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceConfig {
    pub poll_interval_ms: u64,
    pub max_query_concurrency: usize,
    pub max_deep_queries: usize,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
}
impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 500,
            max_query_concurrency: 16,
            max_deep_queries: 2,
            max_request_bytes: 256 * 1024,
            max_response_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalConfig {
    pub max_items: usize,
    pub max_context_bytes: usize,
    pub query_timeout_ms: u64,
}
impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            max_items: 8,
            max_context_bytes: 6400,
            query_timeout_ms: 1500,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryStorage {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectionConfig {
    pub enabled: bool,
    pub poll_interval_ms: u64,
    pub batch_size: i64,
}
impl Default for ProjectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_ms: 5000,
            batch_size: 20,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub storage: MemoryStorage,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub agent: AgentConfig,
    pub embedding: EmbeddingConfig,
    pub service: ServiceConfig,
    pub retrieval: RetrievalConfig,
    pub projection: ProjectionConfig,
}

#[derive(Debug, Clone)]
pub struct MemoryLocation {
    pub enabled: bool,
    pub path: PathBuf,
    pub task_path: PathBuf,
    pub service: ServiceConfig,
    pub retrieval: RetrievalConfig,
}

fn document(path: &Path) -> Result<toml::Value> {
    Ok(toml::from_str(&std::fs::read_to_string(expand(path)?)?)?)
}
fn expand(path: &Path) -> Result<PathBuf> {
    Ok(if let Ok(relative) = path.strip_prefix("~") {
        dirs::home_dir()
            .context("home directory unavailable")?
            .join(relative)
    } else {
        path.to_owned()
    })
}

impl MemoryLocation {
    pub fn load(path: &Path) -> Result<Self> {
        Self::from_value(&document(path)?)
    }

    fn from_value(value: &toml::Value) -> Result<Self> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct MemorySection {
            enabled: bool,
            storage: MemoryStorage,
            service: ServiceConfig,
            retrieval: RetrievalConfig,
        }
        ensure!(
            value
                .get("schema_version")
                .and_then(toml::Value::as_integer)
                == Some(1),
            "unsupported task config schema_version"
        );
        let task: MemoryStorage = value
            .get("storage")
            .context("missing task storage")?
            .clone()
            .try_into()?;
        let task_path = expand(&task.path.context("missing task storage.path")?)?;
        let memory: MemorySection = value
            .get("memory")
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::default()))
            .try_into()?;
        let path = expand(
            &memory
                .storage
                .path
                .unwrap_or_else(|| task_path.with_file_name("memory.sqlite3")),
        )?;
        ensure!(
            task_path.is_absolute() && path.is_absolute() && path.file_name().is_some(),
            "memory and task storage paths must be absolute files"
        );
        ensure!(
            path != task_path
                && (!path.exists()
                    || !task_path.exists()
                    || path.canonicalize()? != task_path.canonicalize()?),
            "memory storage must differ from task storage"
        );
        ensure!(
            (1..=100).contains(&memory.retrieval.max_items)
                && (128..=65536).contains(&memory.retrieval.max_context_bytes)
                && (1..=30000).contains(&memory.retrieval.query_timeout_ms),
            "invalid: retrieval budgets"
        );
        Ok(Self {
            enabled: memory.enabled,
            path,
            task_path,
            service: memory.service,
            retrieval: memory.retrieval,
        })
    }
}

impl MemoryConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let value = document(path)?;
        let location = MemoryLocation::from_value(&value)?;
        let mut config: Self = value
            .get("memory")
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::default()))
            .try_into()?;
        config.storage.path = Some(location.path);
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        for provider in self.providers.values() {
            let url = reqwest::Url::parse(&provider.base_url)?;
            ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "invalid: provider base_url must be HTTP(S) without credentials, query or fragment"
            );
            ensure!(
                (1..=64).contains(&provider.max_in_flight),
                "invalid: provider concurrency"
            );
        }
        let provider = self
            .providers
            .get(&self.agent.provider)
            .context("missing memory Agent provider")?;
        ensure!(
            provider.protocol == ProviderProtocol::Openai,
            "Agent requires an OpenAI-compatible provider"
        );
        ensure!(
            !self.agent.model.trim().is_empty(),
            "missing memory Agent model"
        );
        ensure!(
            !(self.agent.model == "gpt-6-astra" || self.agent.model.starts_with("gpt-6-astra-"))
                || self.agent.api == ModelApi::Responses,
            "GPT-6 Astra tool calling requires the Responses API"
        );
        ensure!(
            (1..=32).contains(&self.agent.max_concurrent_loops)
                && (1..=32).contains(&self.agent.max_extraction_loops_per_project),
            "invalid: Agent concurrency"
        );
        ensure!(
            (1..=64).contains(&self.agent.max_steps)
                && (1..=128).contains(&self.agent.max_tool_calls)
                && (4096..=1024 * 1024).contains(&self.agent.max_context_bytes)
                && (1..=32768).contains(&self.agent.max_output_tokens),
            "invalid: Agent budgets"
        );
        ensure!(
            (1..=600).contains(&self.agent.request_timeout_seconds)
                && (1..=1800).contains(&self.agent.task_timeout_seconds)
                && self.agent.lease_seconds > self.agent.task_timeout_seconds
                && self.agent.lease_seconds <= 3600
                && (1..=10).contains(&self.agent.max_attempts),
            "invalid: Agent timeouts or retries"
        );
        ensure!(
            (1..=64).contains(&self.service.max_query_concurrency)
                && (1..=16).contains(&self.service.max_deep_queries)
                && (10..=60000).contains(&self.service.poll_interval_ms)
                && (1024..=4 * 1024 * 1024).contains(&self.service.max_request_bytes)
                && (1024..=8 * 1024 * 1024).contains(&self.service.max_response_bytes),
            "invalid: service limits"
        );
        ensure!(
            (100..=60000).contains(&self.projection.poll_interval_ms)
                && (1..=100).contains(&self.projection.batch_size),
            "invalid projection limits"
        );
        if self.embedding.enabled {
            ensure!(
                self.providers.contains_key(&self.embedding.provider),
                "missing memory embedding provider"
            );
            ensure!(
                !self.embedding.model.trim().is_empty()
                    && self
                        .embedding
                        .dimensions
                        .is_none_or(|n| (1..=16384).contains(&n))
                    && (1..=64).contains(&self.embedding.batch_size)
                    && (1..=120).contains(&self.embedding.request_timeout_seconds),
                "invalid: embedding settings"
            );
        }
        Ok(())
    }
}
