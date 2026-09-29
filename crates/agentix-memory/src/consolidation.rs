use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{Actor, Memory, MemoryInput, MemoryStore, Status, WorkKind, WorkLease, queue, store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAction {
    Create,
    Merge,
    Supersede,
    Conflict,
    Discard,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationDecision {
    pub candidate: usize,
    pub action: DecisionAction,
    pub target: Option<String>,
    pub expected_revision: Option<i64>,
    pub content: Option<MemoryInput>,
    pub reason: String,
}

impl MemoryStore {
    /// Apply the entire proposal and finish its fenced work item in one transaction.
    #[allow(clippy::too_many_lines)] // Keep the atomic mutation sequence visible together.
    pub async fn complete_consolidation(
        &self,
        lease: &WorkLease,
        decisions: Vec<ConsolidationDecision>,
        now: i64,
    ) -> Result<Vec<Memory>> {
        ensure!(
            lease.kind == WorkKind::Consolidate,
            "invalid consolidation task"
        );
        let candidates: Vec<MemoryInput> = serde_json::from_value(lease.payload.clone())?;
        ensure!(
            decisions.len() == candidates.len(),
            "every candidate needs a consolidation decision"
        );
        let mut indices = HashSet::new();
        let mut changed = Vec::new();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        queue::fence(&mut tx, lease, now).await?;
        for decision in &decisions {
            ensure!(
                indices.insert(decision.candidate),
                "duplicate candidate decision"
            );
            let candidate = candidates
                .get(decision.candidate)
                .context("invalid candidate index")?;
            ensure!(
                !decision.reason.trim().is_empty() && decision.reason.len() <= 2048,
                "consolidation reason required"
            );
            if decision.action == DecisionAction::Discard {
                ensure!(
                    decision.target.is_none() && decision.content.is_none(),
                    "discard cannot mutate memory"
                );
                continue;
            }
            let content = decision.content.as_ref().unwrap_or(candidate);
            content.validate(Actor::Agent)?;
            ensure!(
                candidate
                    .evidence
                    .iter()
                    .all(|e| content.evidence.contains(e)),
                "consolidation must retain candidate evidence"
            );
            store::validate_evidence(&mut tx, &lease.project_id, content).await?;
            if decision.action == DecisionAction::Create {
                ensure!(
                    decision.target.is_none() && decision.expected_revision.is_none(),
                    "create cannot target an existing memory"
                );
                let memory = new_memory(&lease.project_id, content.clone(), &decision.reason, now);
                store::save(&mut tx, &memory).await?;
                changed.push(memory);
                continue;
            }
            let id = decision
                .target
                .as_deref()
                .context("missing consolidation target")?;
            let mut prior = store::load(&mut tx, &lease.project_id, id).await?;
            ensure!(
                Some(prior.revision) == decision.expected_revision && prior.status.searchable(),
                "conflict: consolidation target changed"
            );
            ensure!(
                prior.actor != Actor::Human || decision.action == DecisionAction::Conflict,
                "conflict: preserve human-authored memory; report a conflict instead"
            );
            prior.revision += 1;
            prior.updated_at = now;
            prior.reason.clone_from(&decision.reason);
            match decision.action {
                DecisionAction::Merge => {
                    prior.content = content.clone();
                    prior.actor = Actor::Agent;
                }
                DecisionAction::Archive => {
                    prior.status = Status::Archived;
                    prior.actor = Actor::Agent;
                }
                DecisionAction::Supersede | DecisionAction::Conflict => {
                    let mut new =
                        new_memory(&lease.project_id, content.clone(), &decision.reason, now);
                    if decision.action == DecisionAction::Supersede {
                        new.supersedes = Some(prior.id.clone());
                        prior.superseded_by = Some(new.id.clone());
                        prior.status = Status::Superseded;
                    } else {
                        new.status = Status::Conflicted;
                        prior.status = Status::Conflicted;
                    }
                    store::save(&mut tx, &new).await?;
                    changed.push(new);
                }
                DecisionAction::Create | DecisionAction::Discard => unreachable!(),
            }
            store::save(&mut tx, &prior).await?;
            changed.push(prior);
        }
        queue::finish(&mut tx, lease, &serde_json::to_value(decisions)?).await?;
        tx.commit().await?;
        Ok(changed)
    }
}

fn new_memory(project: &str, content: MemoryInput, reason: &str, now: i64) -> Memory {
    Memory {
        id: format!("mem_{}", uuid::Uuid::now_v7().simple()),
        project_id: project.into(),
        revision: 1,
        status: Status::Active,
        actor: Actor::Agent,
        created_at: now,
        updated_at: now,
        reason: reason.into(),
        supersedes: None,
        superseded_by: None,
        content,
    }
}
