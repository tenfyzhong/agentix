use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub instance_id: String,
    pub receipt_id: String,
    pub sequence: i64,
    pub project_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub revision: i64,
    pub job_id: Option<String>,
    pub recorded_at: i64,
    pub messages: Vec<SourceMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Agent,
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    UserDecision,
    UserAssertion,
    Observation,
    Inference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Conflicted,
    Superseded,
    Archived,
    Forgotten,
    Invalidated,
}

impl Status {
    pub(crate) fn searchable(self) -> bool {
        matches!(self, Self::Active | Self::Conflicted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub receipt_id: String,
    pub message_id: String,
    pub quote: String,
}

/// Identity excludes the value, wording and ingestion time of a fact version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub entity: String,
    pub attribute: String,
    pub qualifiers: Vec<FactQualifier>,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactQualifier {
    pub name: String,
    pub value: String,
}

impl Fact {
    pub(crate) fn key(&self) -> Result<String> {
        let mut qualifiers: Vec<_> = self
            .qualifiers
            .iter()
            .map(|q| (q.name.trim().to_lowercase(), q.value.trim()))
            .collect();
        qualifiers.sort_unstable();
        Ok(serde_json::to_string(&(
            self.entity.trim().to_lowercase(),
            self.attribute.trim().to_lowercase(),
            qualifiers,
        ))?)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.entity.trim().is_empty()
                && self.entity.len() <= 512
                && !self.attribute.trim().is_empty()
                && self.attribute.len() <= 256
                && !self.value.trim().is_empty()
                && self.value.len() <= 8192
                && self.qualifiers.len() <= 16
                && self.qualifiers.iter().all(|q| !q.name.trim().is_empty()
                    && q.name.len() <= 128
                    && !q.value.trim().is_empty()
                    && q.value.len() <= 512),
            "invalid: fact identity or value"
        );
        let names: std::collections::HashSet<_> = self
            .qualifiers
            .iter()
            .map(|q| q.name.trim().to_lowercase())
            .collect();
        ensure!(
            names.len() == self.qualifiers.len(),
            "duplicate fact qualifier"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<Fact>,
    pub title: String,
    pub conclusion: String,
    pub rationale: String,
    pub scope: String,
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub valid_until: Option<i64>,
    pub tags: Vec<String>,
    pub kind: Kind,
    pub evidence: Vec<Evidence>,
}

impl MemoryInput {
    pub(crate) fn validate(&self, actor: Actor) -> Result<()> {
        if let Some(fact) = &self.fact {
            fact.validate()?;
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 60 * 1024,
            "invalid: atomic memory exceeds 60 KiB"
        );
        for (value, limit) in [
            (&self.title, 512),
            (&self.conclusion, 8192),
            (&self.scope, 1024),
        ] {
            ensure!(
                !value.trim().is_empty() && value.len() <= limit,
                "invalid: memory field size"
            );
        }
        ensure!(
            self.rationale.len() <= 8192,
            "invalid: memory rationale size"
        );
        ensure!(
            self.conditions.len() <= 16
                && self
                    .conditions
                    .iter()
                    .all(|c| !c.trim().is_empty() && c.len() <= 1024),
            "invalid: memory conditions"
        );
        ensure!(
            self.tags.len() <= 32 && self.tags.iter().all(|t| !t.is_empty() && t.len() <= 128),
            "invalid: memory tags"
        );
        ensure!(
            self.evidence.len() <= 16 && (actor == Actor::Human || !self.evidence.is_empty()),
            "invalid: memory evidence required"
        );
        for evidence in &self.evidence {
            ensure!(
                !evidence.quote.trim().is_empty() && evidence.quote.len() <= 2048,
                "invalid: evidence quote size"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub project_id: String,
    pub revision: i64,
    pub status: Status,
    pub actor: Actor,
    pub created_at: i64,
    pub updated_at: i64,
    pub reason: String,
    pub supersedes: Option<String>,
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    pub content: MemoryInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexPage {
    pub indexed: usize,
    pub next_cursor: String,
    pub complete: bool,
}
