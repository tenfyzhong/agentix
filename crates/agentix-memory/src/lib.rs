//! Independent project memory domain, persistence and retrieval.
mod agent_loop;
pub use agent_loop::{AgentLoop, LoopResult, ToolSet};
mod config;
mod domain;
mod providers;
mod retrieval;
mod store;
mod vectors;
pub use providers::{
    HttpEmbedding, HttpModel, HttpProvider, Message, Model, ModelReply, ModelRequest, TokenUsage,
    ToolCall, ToolDefinition,
};

pub use config::{
    AgentConfig, EmbeddingConfig, MemoryConfig, MemoryLocation, ModelApi, ProjectionConfig,
    ProviderConfig, ProviderProtocol, RetrievalConfig, ServiceConfig,
};
pub use domain::{
    Actor, Evidence, IndexPage, Kind, Memory, MemoryInput, Source, SourceMessage, Status,
};
pub use store::{MemoryStore, SourceSummary};

mod tools;
pub use tools::ProjectTools;

mod queue;
pub use queue::{WorkCounts, WorkKind, WorkLease};

mod consolidation;
pub use consolidation::{ConsolidationDecision, DecisionAction};

mod worker;
pub use worker::{ExtractionGate, MemoryWorker, ProjectRepository, TriageDecision};

mod embedding_index;
pub use embedding_index::{EmbeddingIndex, EmbeddingProfile, SearchResult, SemanticRetrieval};

#[cfg(unix)]
mod ipc;
#[cfg(unix)]
pub use ipc::{IpcClient, IpcServer, RequestHandler};

mod context;
mod maintenance;
pub use context::{ContextPacket, MemoryRef, context_preview};

#[cfg(unix)]
mod api;
#[cfg(unix)]
pub use api::{MemoryApi, MemoryRequest};

mod deep_query;
pub use deep_query::{DeepAnswer, DeepQuery};

mod projection;
pub use projection::{MemoryProjection, ProjectionPage};

mod review;
