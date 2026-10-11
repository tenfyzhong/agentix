use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IssueStatus {
    Todo,
    Active,
    PendingReview,
    Completed,
    Cancelled,
}

impl IssueStatus {
    pub const ALL: [Self; 5] = [
        Self::Todo,
        Self::Active,
        Self::PendingReview,
        Self::Completed,
        Self::Cancelled,
    ];

    #[must_use]
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

impl fmt::Display for IssueStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Todo => "TODO",
            Self::Active => "ACTIVE",
            Self::PendingReview => "PENDING_REVIEW",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub project_id: String,
    pub milestone_id: Option<String>,
    pub title: String,
    pub name: String,
    pub sequence: u64,
    #[serde(default)]
    pub review_content_hash: String,
    pub content_hash: String,
    pub status: IssueStatus,
    pub review_reason: Option<String>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub pending_review_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub cancelled_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub document_path: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MilestoneStatus {
    #[default]
    Planned,
    Active,
    Completed,
    Cancelled,
}

impl MilestoneStatus {
    #[must_use]
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Milestone {
    #[serde(default)]
    pub status: MilestoneStatus,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub cancelled_at: Option<i64>,
    #[serde(default)]
    pub archived_at: Option<i64>,
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub name: String,
    pub sequence: u64,
    pub goal: String,
    pub order: i64,
    pub due_date: Option<String>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub document_path: String,
}

/// Authored Issue sections read from the vault, never persisted in an Issue row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssueContent {
    pub requirement: String,
    pub acceptance: String,
    pub notes: String,
}
