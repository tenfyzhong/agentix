use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    Actor, Evidence, Memory, MemoryInput, MemoryStore, Source, Status, WorkKind, WorkLease, queue,
    store,
};

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
    #[serde(default)]
    pub related: Vec<RelatedDecision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelatedAction {
    Keep,
    Supersede,
    Conflict,
    Forget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelatedDecision {
    pub id: String,
    pub revision: i64,
    pub action: RelatedAction,
    pub reason: String,
    pub retained: Option<MemoryInput>,
    pub forget_request: Option<Evidence>,
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
        let seed: Option<Memory> = lease
            .payload
            .get("compact")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()?;
        let candidates: Vec<MemoryInput> = if let Some(seed) = &seed {
            vec![seed.content.clone()]
        } else {
            serde_json::from_value(lease.payload.clone())?
        };
        ensure!(
            decisions.len() == candidates.len(),
            "every candidate needs a consolidation decision"
        );
        let mut indices = HashSet::new();
        let mut changed = Vec::new();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        queue::fence(&mut tx, lease, now).await?;
        if let Some(seed) = &seed {
            let current = store::load(&mut tx, &lease.project_id, &seed.id).await?;
            ensure!(
                current.revision == seed.revision
                    && current.actor == Actor::Agent
                    && current.status.searchable(),
                "conflict: compaction seed changed"
            );
            ensure!(
                decisions.iter().all(|d| {
                    d.action == DecisionAction::Discard
                        || (d.action != DecisionAction::Create
                            && d.target.as_deref() == Some(seed.id.as_str()))
                        || (d.action == DecisionAction::Merge
                            && d.target.is_some()
                            && d.related.iter().any(|r| {
                                r.id == seed.id
                                    && r.revision == seed.revision
                                    && r.action == RelatedAction::Supersede
                                    && r.retained.is_none()
                            }))
                }),
                "compaction must revise its seed or merge into an existing target and retire the exact seed without a retained duplicate"
            );
            ensure!(
                decisions
                    .iter()
                    .all(|d| d.related.iter().all(|r| r.action != RelatedAction::Forget)),
                "compaction cannot authorize forgetting"
            );
        }
        // All related assessments refer to the original transaction snapshot.
        // A read-only keep can follow a sibling candidate's mutation of that record.
        for related in decisions.iter().flat_map(|decision| &decision.related) {
            let prior = store::load(&mut tx, &lease.project_id, &related.id).await?;
            ensure!(
                prior.revision == related.revision && prior.status.searchable(),
                "conflict: related memory changed"
            );
        }
        for decision in &decisions {
            ensure!(
                decision.action != DecisionAction::Archive,
                "automatic archival requires repository review with a validated citation"
            );
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
            let mut related_ids = HashSet::new();
            ensure!(
                decision.related.len() <= 16
                    && decision.related.iter().all(|r| related_ids.insert(&r.id)),
                "duplicate or oversized related assessments"
            );
            if decision.action == DecisionAction::Discard {
                ensure!(
                    decision.target.is_none() && decision.content.is_none(),
                    "discard cannot mutate memory"
                );
                for related in &decision.related {
                    apply_related(&mut tx, lease, candidate, related, None, &mut changed, now)
                        .await?;
                }
                continue;
            }
            let mut content = decision.content.as_ref().unwrap_or(candidate).clone();
            if seed.is_some() {
                preserve_evidence(&mut content, candidate)?;
            }
            content.validate(Actor::Agent)?;
            if candidate.fact.is_some() && seed.is_none() {
                ensure!(
                    crate::facts::same_value(&content, candidate)?,
                    "consolidation cannot change the extracted fact identity or value"
                );
            }
            ensure!(
                candidate.evidence.iter().all(|original| {
                    content.evidence.iter().any(|retained| {
                        retained.receipt_id == original.receipt_id
                            && retained.message_id == original.message_id
                            && retained.quote.contains(&original.quote)
                    })
                }),
                "consolidation must retain candidate evidence"
            );
            store::validate_evidence(&mut tx, &lease.project_id, &content).await?;
            if decision.action == DecisionAction::Create {
                ensure!(
                    decision.target.is_none() && decision.expected_revision.is_none(),
                    "create cannot target an existing memory"
                );
                let memory = new_memory(&lease.project_id, content.clone(), &decision.reason, now)?;
                store::save(&mut tx, &memory).await?;
                for related in &decision.related {
                    apply_related(
                        &mut tx,
                        lease,
                        candidate,
                        related,
                        Some(&memory),
                        &mut changed,
                        now,
                    )
                    .await?;
                }
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
            if prior.content.fact.is_some() || content.fact.is_some() {
                ensure!(
                    crate::facts::same_identity(&prior.content, &content)?,
                    "consolidation cannot combine different facts"
                );
                if decision.action == DecisionAction::Merge {
                    ensure!(
                        crate::facts::same_value(&prior.content, &content)?,
                        "changed fact values require supersede, not merge"
                    );
                } else if decision.action == DecisionAction::Supersede {
                    crate::facts::validate_replacement(&mut tx, &prior, &content).await?;
                }
            }
            prior.revision += 1;
            prior.updated_at = now;
            prior.reason.clone_from(&decision.reason);
            match decision.action {
                DecisionAction::Merge => {
                    prior.content = content.clone();
                    let original = store::load(&mut tx, &lease.project_id, id).await?;
                    preserve_evidence(&mut prior.content, &original.content)?;
                    prior.actor = Actor::Agent;
                }
                DecisionAction::Archive => {
                    prior.status = Status::Archived;
                    prior.actor = Actor::Agent;
                }
                DecisionAction::Supersede | DecisionAction::Conflict => {
                    let mut new =
                        new_memory(&lease.project_id, content.clone(), &decision.reason, now)?;
                    if decision.action == DecisionAction::Supersede {
                        new.supersedes = Some(prior.id.clone());
                        prior.superseded_by = Some(new.id.clone());
                        prior.status = Status::Superseded;
                    } else {
                        new.status = Status::Conflicted;
                        prior.status = Status::Conflicted;
                    }
                    if decision.action == DecisionAction::Supersede {
                        store::save(&mut tx, &prior).await?;
                    }
                    store::save(&mut tx, &new).await?;
                    changed.push(new);
                }
                DecisionAction::Create | DecisionAction::Discard => unreachable!(),
            }
            if decision.action != DecisionAction::Supersede {
                store::save(&mut tx, &prior).await?;
            }
            let anchor = if matches!(
                decision.action,
                DecisionAction::Supersede | DecisionAction::Conflict
            ) {
                changed.last().context("missing new memory")?.clone()
            } else {
                prior.clone()
            };
            for related in &decision.related {
                ensure!(
                    related.id != id,
                    "primary target cannot also be a related target"
                );
                apply_related(
                    &mut tx,
                    lease,
                    candidate,
                    related,
                    Some(&anchor),
                    &mut changed,
                    now,
                )
                .await?;
            }
            changed.push(prior);
        }
        crate::facts::check_settled(&mut tx, &changed).await?;
        queue::finish(&mut tx, lease, &serde_json::to_value(decisions)?).await?;
        if let Some(seed) = &seed {
            let current = store::load(&mut tx, &lease.project_id, &seed.id).await?;
            crate::compaction::observed(&mut tx, &current, now).await?;
            for memory in &changed {
                crate::compaction::observed(&mut tx, memory, now).await?;
            }
        }
        if seed.is_none() {
            for memory in changed.iter().filter(|m| m.content.fact.is_some()) {
                crate::compaction::observed(&mut tx, memory, now).await?;
            }
        }
        tx.commit().await?;
        if !changed.is_empty() {
            self.notify_change(&lease.project_id);
        } else if seed.is_some() {
            self.notify_compaction(&lease.project_id);
        }
        Ok(changed)
    }
}

fn covers(content: &MemoryInput, original: &Evidence) -> bool {
    content.evidence.iter().any(|retained| {
        retained.receipt_id == original.receipt_id
            && retained.message_id == original.message_id
            && retained.quote.contains(&original.quote)
    })
}

pub(crate) fn preserve_evidence(content: &mut MemoryInput, prior: &MemoryInput) -> Result<()> {
    for evidence in &prior.evidence {
        if !covers(content, evidence) {
            content.evidence.push(evidence.clone());
        }
    }
    content.validate(Actor::Agent)
}

#[allow(clippy::too_many_arguments)]
async fn apply_related(
    conn: &mut sqlx::SqliteConnection,
    lease: &WorkLease,
    candidate: &MemoryInput,
    decision: &RelatedDecision,
    anchor: Option<&Memory>,
    changed: &mut Vec<Memory>,
    now: i64,
) -> Result<()> {
    ensure!(
        !decision.reason.trim().is_empty() && decision.reason.len() <= 2048,
        "related assessment reason required"
    );
    if decision.action == RelatedAction::Keep {
        ensure!(
            decision.retained.is_none() && decision.forget_request.is_none(),
            "keep cannot mutate memory"
        );
        return Ok(());
    }
    let mut prior = store::load(conn, &lease.project_id, &decision.id).await?;
    ensure!(
        prior.revision == decision.revision && prior.status.searchable(),
        "conflict: related memory changed"
    );
    ensure!(
        prior.actor != Actor::Human || decision.action == RelatedAction::Conflict,
        "conflict: preserve human-authored memory"
    );
    ensure!(
        decision.forget_request.is_none() || decision.action == RelatedAction::Forget,
        "forget evidence only applies to forgetting"
    );
    if let Some(retained) = &decision.retained {
        ensure!(
            decision.action == RelatedAction::Supersede,
            "retained facts require partial replacement"
        );
        retained.validate(Actor::Agent)?;
        store::validate_evidence(conn, &lease.project_id, retained).await?;
    }
    match decision.action {
        RelatedAction::Supersede => {
            let anchor = anchor.context("replacement requires a current memory")?;
            if prior.content.fact.is_some() {
                crate::facts::validate_replacement(conn, &prior, &anchor.content).await?;
            }
            ensure!(
                prior
                    .content
                    .evidence
                    .iter()
                    .all(|e| covers(&anchor.content, e)
                        || decision
                            .retained
                            .as_ref()
                            .is_some_and(|retained| covers(retained, e))),
                "replacement must preserve prior evidence in current or retained facts"
            );
            if let Some(retained) = &decision.retained {
                let mut remainder =
                    new_memory(&lease.project_id, retained.clone(), &decision.reason, now)?;
                remainder.supersedes = Some(prior.id.clone());
                store::save(conn, &remainder).await?;
                changed.push(remainder);
            }
            prior.status = Status::Superseded;
            prior.superseded_by = Some(anchor.id.clone());
        }
        RelatedAction::Conflict => {
            let mut anchor = anchor.context("conflict requires both claims")?.clone();
            if prior.content.fact.is_some() {
                ensure!(
                    crate::facts::same_identity(&prior.content, &anchor.content)?,
                    "conflict must refer to the same fact"
                );
            }
            if anchor.status != Status::Conflicted {
                anchor.revision += 1;
                anchor.status = Status::Conflicted;
                anchor.updated_at = now;
                store::save(conn, &anchor).await?;
                changed.push(anchor);
            }
            prior.status = Status::Conflicted;
        }
        RelatedAction::Forget => {
            validate_forget(conn, lease, candidate, decision, &prior).await?;
            store::suppress_history(conn, &lease.project_id, &prior.id).await?;
            prior.status = Status::Forgotten;
        }
        RelatedAction::Keep => unreachable!(),
    }
    prior.revision += 1;
    prior.updated_at = now;
    prior.reason.clone_from(&decision.reason);
    store::save(conn, &prior).await?;
    changed.push(prior);
    Ok(())
}

async fn validate_forget(
    conn: &mut sqlx::SqliteConnection,
    lease: &WorkLease,
    candidate: &MemoryInput,
    decision: &RelatedDecision,
    prior: &Memory,
) -> Result<()> {
    let auth = decision
        .forget_request
        .as_ref()
        .context("forget requires an explicit current user request")?;
    ensure!(
        auth.receipt_id == lease.receipt_id
            && Some(auth.message_id.as_str())
                == lease.payload["message_id"].as_str().or_else(|| candidate
                    .evidence
                    .iter()
                    .find(|e| e.receipt_id == lease.receipt_id)
                    .map(|e| e.message_id.as_str())),
        "forget request must belong to current source"
    );
    let data: String =
        sqlx::query_scalar("SELECT data FROM sources WHERE receipt_id=? AND project_id=?")
            .bind(&auth.receipt_id)
            .bind(&lease.project_id)
            .fetch_one(&mut *conn)
            .await?;
    let source: Source = serde_json::from_str(&data)?;
    ensure!(
        source.recorded_at >= prior.updated_at
            && source.messages.iter().any(|m| m.id == auth.message_id
                && m.role == "user"
                && m.text.trim() == auth.quote.trim())
            && candidate.evidence.iter().any(|e| e == auth)
            && explicit_forget_request(&auth.quote, prior),
        "forget requires a dated explicit user request naming the memory"
    );

    Ok(())
}

fn explicit_forget_request(text: &str, memory: &Memory) -> bool {
    let text = text.trim().trim_end_matches(['.', '\u{3002}']);
    [&memory.id, &memory.content.title]
        .into_iter()
        .any(|target| {
            text.eq_ignore_ascii_case(&format!("forget memory {target}"))
                || text == format!("\u{5fd8}\u{8bb0}\u{8bb0}\u{5fc6} {target}")
        })
}

pub(crate) fn new_memory(
    project: &str,
    content: MemoryInput,
    reason: &str,
    now: i64,
) -> Result<Memory> {
    Ok(Memory {
        id: crate::ids::new_id(now)?,
        project_id: project.into(),
        revision: 1,
        status: Status::Active,
        actor: Actor::Agent,
        created_at: now,
        updated_at: now,
        reason: reason.into(),
        supersedes: None,
        superseded_by: None,
        derived_from: Vec::new(),
        content,
    })
}
