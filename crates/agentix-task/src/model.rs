use std::fmt;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_LEASE_SECONDS: i64 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    WaitingUser,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub const ALL: [Self; 7] = [
        Self::Todo,
        Self::InProgress,
        Self::Blocked,
        Self::WaitingUser,
        Self::Done,
        Self::Failed,
        Self::Cancelled,
    ];
    #[must_use]
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
    #[must_use]
    pub const fn allows(self, next: Self) -> bool {
        use TaskStatus::{Blocked, Cancelled, Done, Failed, InProgress, Todo, WaitingUser};
        matches!(
            (self, next),
            (Todo, InProgress | Blocked | WaitingUser | Cancelled)
                | (
                    InProgress,
                    Done | Blocked | WaitingUser | Failed | Cancelled
                )
                | (Blocked, InProgress | WaitingUser | Failed | Cancelled)
                | (WaitingUser, InProgress | Blocked | Failed | Cancelled)
        )
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Todo => "TODO",
            Self::InProgress => "IN_PROGRESS",
            Self::Blocked => "BLOCKED",
            Self::WaitingUser => "WAITING_USER",
            Self::Done => "DONE",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskPhase {
    Planning,
    Executing,
}

impl fmt::Display for TaskPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Planning => "PLANNING",
            Self::Executing => "EXECUTING",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobStatus {
    Active,
    Completed,
    Cancelled,
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Active => "ACTIVE",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub key: String,
    pub name: String,
    pub root: String,
    pub remote: Option<String>,
    pub revision: i64,
    pub created_at: i64,
    #[serde(default)]
    pub archived_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sequence: u64,
    pub goal: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub status: JobStatus,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub cancelled_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub document_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub job_id: String,
    pub title: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sequence: u64,
    pub status: TaskStatus,
    #[serde(default)]
    pub phase: Option<TaskPhase>,
    pub revision: i64,
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    pub reason: Option<String>,
    pub dependencies: Vec<String>,
    pub current_plan: Option<String>,
    pub last_executor: Option<String>,
    pub last_session: Option<String>,
    pub delegated_by: Option<String>,
    pub system_block: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub task_id: String,
    pub version: i64,
    pub path: String,
    pub hash: String,
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    /// Pending publication survives a crash between the database commit and sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub task_id: String,
    pub executor_ref: String,
    pub session_ref: String,
    pub delegated_by: Option<String>,
    pub token: String,
    pub lease_expires_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub document_sequences: std::collections::BTreeMap<String, u64>,
    pub projects: Vec<Project>,
    pub jobs: Vec<Job>,
    pub tasks: Vec<Task>,
    pub plans: Vec<Plan>,
    pub leases: Vec<Lease>,
}

impl Snapshot {
    pub fn project_index(&self, id: &str) -> Result<usize> {
        resolve(
            self.projects
                .iter()
                .map(|p| (p.id.as_str(), p.name.as_str())),
            id,
        )
    }
    pub fn job_index(&self, id: &str) -> Result<usize> {
        resolve(self.jobs.iter().map(|p| (p.id.as_str(), p.id.as_str())), id)
    }
    pub fn task_index(&self, id: &str) -> Result<usize> {
        resolve(
            self.tasks.iter().map(|p| (p.id.as_str(), p.id.as_str())),
            id,
        )
    }
    pub fn task_result(&self, id: &str) -> Result<Value> {
        let task = &self.tasks[self.task_index(id)?];
        let mut result = serde_json::to_value(task)?;
        result["lease"] = serde_json::to_value(self.leases.iter().find(|l| l.task_id == task.id))?;
        Ok(result)
    }
}

fn resolve<'a>(items: impl Iterator<Item = (&'a str, &'a str)>, query: &str) -> Result<usize> {
    if query.is_empty() {
        bail!("invalid: empty identifier");
    }
    let items: Vec<_> = items.collect();
    if let Some(index) = items.iter().position(|(id, _)| *id == query) {
        return Ok(index);
    }
    let candidates: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, (id, name))| id.starts_with(query) || *name == query)
        .map(|(i, _)| i)
        .collect();
    match candidates.as_slice() {
        [index] => Ok(*index),
        [] => bail!("not_found: {query}"),
        _ => bail!("ambiguous identifier: {query}"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteOptions {
    pub actor_ref: String,
    pub session_ref: Option<String>,
    pub delegated_by: Option<String>,
    pub lease_token: Option<String>,
    pub expected_revision: Option<i64>,
    pub idempotency_key: Option<String>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            actor_ref: "user:cli".into(),
            session_ref: None,
            delegated_by: None,
            lease_token: None,
            expected_revision: None,
            idempotency_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub result: Value,
    pub sequence: i64,
    pub projection_pending: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub sequence: i64,
    pub event_id: String,
    pub project_id: Option<String>,
    pub job_id: Option<String>,
    pub task_id: Option<String>,
    pub actor_ref: String,
    pub session_ref: Option<String>,
    pub delegated_by: Option<String>,
    pub event_type: String,
    pub revision: i64,
    pub occurred_at: i64,
    pub payload: Value,
}

#[must_use]
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::now_v7().simple())
}

pub(crate) fn agent_name(executor: &str) -> Option<&str> {
    let name = executor
        .strip_prefix("agent:")
        .unwrap_or(executor)
        .split(':')
        .next()?;
    match name {
        "codex" | "claude" | "pi" | "omp" => Some(name),
        _ => None,
    }
}
