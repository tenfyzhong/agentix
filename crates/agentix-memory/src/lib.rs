//! Independent project memory domain, persistence and retrieval.
mod agent_loop;
pub use agent_loop::{AgentLoop, LoopResult, ToolInputError, ToolSet};
mod config;
mod domain;
mod id_migration;
mod ids;
mod providers;
mod retrieval;
mod store;
mod vectors;
pub use providers::{
    HttpEmbedding, HttpModel, HttpProvider, Message, Model, ModelReply, ModelRequest,
    ProviderLimits, TokenUsage, ToolCall, ToolDefinition,
};

pub use config::{
    AgentConfig, EmbeddingConfig, MemoryConfig, MemoryLocation, ModelApi, ProjectionConfig,
    ProviderConfig, ProviderProtocol, ReasoningEffort, RetrievalConfig, ServiceConfig,
};
pub use domain::{
    Actor, Evidence, Fact, FactQualifier, IndexPage, Kind, Memory, MemoryInput, Source,
    SourceMessage, Status,
};
pub use store::{MemoryStore, SourceSummary};

mod tools;
pub use tools::ProjectTools;

mod queue;
pub use queue::{WorkCounts, WorkKind, WorkLease};

mod consolidation;
pub use consolidation::{ConsolidationDecision, DecisionAction, RelatedAction, RelatedDecision};
mod fact_compaction;
mod facts;
pub use fact_compaction::{FactCompaction, FactPart};
mod compaction;
pub use compaction::CompactionPage;

mod worker;
pub use worker::{ExtractionGate, MemoryWorker, ProjectRepository, TriageDecision};

mod embedding_index;
pub use embedding_index::{EmbeddingIndex, EmbeddingProfile, SearchResult, SemanticRetrieval};

#[cfg(any(unix, windows))]
mod ipc;
#[cfg(any(unix, windows))]
pub use ipc::{IpcClient, IpcServer, RequestHandler};

mod context;
mod maintenance;
pub use context::{ContextPacket, MemoryRef, context_preview};

#[cfg(any(unix, windows))]
mod api;
#[cfg(any(unix, windows))]
pub use api::{MemoryApi, MemoryRequest};

mod deep_query;
pub use deep_query::{DeepAnswer, DeepQuery};

mod projection;
pub use projection::{MemoryProjection, ProjectionPage};

mod review;
