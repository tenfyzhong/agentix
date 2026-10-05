//! Fenced one-to-many migration of legacy mixed memories into atomic fact versions.
use crate::{
    Actor, DecisionAction, Memory, MemoryInput, MemoryStore, RelatedAction, RelatedDecision,
    Status, WorkKind, WorkLease, compaction, consolidation, facts, queue, store,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactPart {
    pub content: MemoryInput,
    pub action: DecisionAction,
    pub target: Option<String>,
    pub expected_revision: Option<i64>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactCompaction {
    pub parts: Vec<FactPart>,
    pub related: Vec<RelatedDecision>,
    pub reason: String,
}

impl MemoryStore {
    /// Split only after every part and all original evidence validate atomically.
    pub async fn complete_fact_compaction(
        &self,
        lease: &WorkLease,
        proposal: FactCompaction,
        now: i64,
    ) -> Result<Vec<Memory>> {
        ensure!(
            lease.kind == WorkKind::Consolidate,
            "invalid fact compaction work"
        );
        let seed: Memory = serde_json::from_value(
            lease
                .payload
                .get("compact")
                .context("compact seed required")?
                .clone(),
        )?;
        ensure!(
            seed.content.fact.is_none(),
            "atomic seeds use normal fact reconciliation"
        );
        ensure!(
            proposal.parts.len() <= 16
                && proposal.related.len() <= 16
                && !proposal.reason.trim().is_empty()
                && proposal.reason.len() <= 2048,
            "invalid fact compaction proposal"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        queue::fence(&mut tx, lease, now).await?;
        let current = store::load(&mut tx, &lease.project_id, &seed.id).await?;
        ensure!(
            current.revision == seed.revision
                && current.actor == Actor::Agent
                && current.status.searchable(),
            "conflict: compaction seed changed"
        );
        let mut related_ids = std::collections::HashSet::new();
        for assessment in &proposal.related {
            ensure!(
                related_ids.insert(&assessment.id) && assessment.id != seed.id,
                "duplicate related assessment"
            );
            let prior = store::load(&mut tx, &lease.project_id, &assessment.id).await?;
            ensure!(
                prior.revision == assessment.revision && prior.status.searchable(),
                "conflict: related memory changed"
            );
            ensure!(
                matches!(
                    assessment.action,
                    RelatedAction::Keep | RelatedAction::Supersede
                ) && assessment.retained.is_none()
                    && assessment.forget_request.is_none(),
                "split assessments can only keep or supersede whole legacy records"
            );
        }
        let mut origins = vec![seed.id.clone()];
        origins.extend(
            proposal
                .related
                .iter()
                .filter(|r| r.action == RelatedAction::Supersede)
                .map(|r| r.id.clone()),
        );
        let mut changed = Vec::new();
        let mut keys = std::collections::HashSet::new();
        for part in &proposal.parts {
            part.content.validate(Actor::Agent)?;
            let fact = part
                .content
                .fact
                .as_ref()
                .context("every split part requires one atomic fact")?;
            ensure!(
                keys.insert(fact.key()?),
                "duplicate fact identity in split parts"
            );
            store::validate_evidence(&mut tx, &lease.project_id, &part.content).await?;
            let memory = apply_part(&mut tx, lease, part, &mut changed, &origins, now).await?;
            sqlx::query("INSERT OR IGNORE INTO memory_fact_origins(memory_id,source_memory_id,source_revision) VALUES(?,?,?)")
                .bind(&memory.id).bind(&seed.id).bind(seed.revision).execute(&mut *tx).await?;
        }
        if !proposal.parts.is_empty() {
            retire(&mut tx, current, &changed, &proposal.reason, now).await?;
            changed.push(store::load(&mut tx, &lease.project_id, &seed.id).await?);
        }
        for assessment in &proposal.related {
            if assessment.action == RelatedAction::Supersede {
                let prior = store::load(&mut tx, &lease.project_id, &assessment.id).await?;
                retire(&mut tx, prior, &changed, &assessment.reason, now).await?;
                changed.push(store::load(&mut tx, &lease.project_id, &assessment.id).await?);
            }
        }
        facts::check_settled(&mut tx, &changed).await?;
        for memory in &changed {
            compaction::observed(&mut tx, memory, now).await?;
        }
        let seed_current = store::load(&mut tx, &lease.project_id, &seed.id).await?;
        compaction::observed(&mut tx, &seed_current, now).await?;
        queue::finish(&mut tx, lease, &serde_json::to_value(&proposal)?).await?;
        tx.commit().await?;
        self.notify_change(&lease.project_id);
        Ok(changed)
    }
}

async fn apply_part(
    conn: &mut SqliteConnection,
    lease: &WorkLease,
    part: &FactPart,
    changed: &mut Vec<Memory>,
    origins: &[String],
    now: i64,
) -> Result<Memory> {
    ensure!(
        !part.reason.trim().is_empty() && part.reason.len() <= 2048,
        "fact part reason required"
    );
    if part.action == DecisionAction::Create {
        ensure!(
            part.target.is_none() && part.expected_revision.is_none(),
            "create cannot target existing fact"
        );
        let mut memory =
            consolidation::new_memory(&lease.project_id, part.content.clone(), &part.reason, now)?;
        memory.derived_from = origins.to_vec();
        store::save(conn, &memory).await?;
        changed.push(memory.clone());
        return Ok(memory);
    }
    ensure!(
        matches!(
            part.action,
            DecisionAction::Merge | DecisionAction::Supersede | DecisionAction::Conflict
        ),
        "split parts must reconcile an atomic fact"
    );
    let mut prior = store::load(
        conn,
        &lease.project_id,
        part.target.as_deref().context("part target required")?,
    )
    .await?;
    ensure!(
        Some(prior.revision) == part.expected_revision && prior.status.searchable(),
        "conflict: part target changed"
    );
    ensure!(
        prior.actor != Actor::Human || part.action == DecisionAction::Conflict,
        "preserve human-authored fact"
    );
    ensure!(
        prior.content.fact.is_some() && facts::same_identity(&prior.content, &part.content)?,
        "split cannot merge unrelated facts or legacy records"
    );
    prior.revision += 1;
    prior.updated_at = now;
    prior.reason.clone_from(&part.reason);
    if part.action == DecisionAction::Merge {
        ensure!(
            facts::same_value(&prior.content, &part.content)?,
            "changed fact values require supersede"
        );
        let original = prior.content.clone();
        prior.content = part.content.clone();
        consolidation::preserve_evidence(&mut prior.content, &original)?;
        for origin in origins {
            if !prior.derived_from.contains(origin) {
                prior.derived_from.push(origin.clone());
            }
        }
        store::save(conn, &prior).await?;
        changed.push(prior.clone());
        return Ok(prior);
    }
    let mut memory =
        consolidation::new_memory(&lease.project_id, part.content.clone(), &part.reason, now)?;
    memory.derived_from = origins.to_vec();
    if part.action == DecisionAction::Supersede {
        facts::validate_replacement(conn, &prior, &part.content).await?;
        prior.status = Status::Superseded;
        prior.superseded_by = Some(memory.id.clone());
        memory.supersedes = Some(prior.id.clone());
    } else {
        prior.status = Status::Conflicted;
        memory.status = Status::Conflicted;
    }
    store::save(conn, &prior).await?;
    store::save(conn, &memory).await?;
    changed.extend([prior, memory.clone()]);
    Ok(memory)
}

async fn retire(
    conn: &mut SqliteConnection,
    mut prior: Memory,
    changed: &[Memory],
    reason: &str,
    now: i64,
) -> Result<()> {
    ensure!(
        prior.actor == Actor::Agent && prior.content.fact.is_none(),
        "only legacy agent records can be split"
    );
    let parts: Vec<_> = changed
        .iter()
        .filter(|m| m.content.fact.is_some())
        .collect();
    ensure!(
        !parts.is_empty()
            && prior
                .content
                .evidence
                .iter()
                .all(|quote| parts
                    .iter()
                    .any(|part| part
                        .content
                        .evidence
                        .iter()
                        .any(|e| e.receipt_id == quote.receipt_id
                            && e.message_id == quote.message_id
                            && e.quote.contains(&quote.quote)))),
        "split must preserve every original quotation in its atomic parts"
    );
    for part in &parts {
        sqlx::query("INSERT OR IGNORE INTO memory_fact_origins(memory_id,source_memory_id,source_revision) VALUES(?,?,?)")
            .bind(&part.id).bind(&prior.id).bind(prior.revision).execute(&mut *conn).await?;
    }
    prior.revision += 1;
    prior.status = Status::Superseded;
    prior.superseded_by = Some(parts[0].id.clone());
    prior.updated_at = now;
    prior.reason = reason.into();
    store::save(conn, &prior).await
}
