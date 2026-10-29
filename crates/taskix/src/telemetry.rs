//! Read-only, bounded background snapshots. HTTP requests never access `SQLite`.
use agentix_metrics::{Metrics, Snapshot};
use anyhow::{Result, ensure};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    path::{Path, PathBuf},
    time::Duration,
};

const BATCH: i64 = 1000;
// Fixed buckets keep per-kind storage bounded and cover subsecond routing through long calls.
const JEV_DURATION_BUCKETS: [f64; 18] = [
    0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0,
    1800.0, 3600.0,
];
const STATES: &[&str] = &[
    "todo",
    "active",
    "in_progress",
    "waiting_user",
    "blocked",
    "pending_review",
    "done",
    "completed",
    "failed",
    "cancelled",
    "other",
];
const KINDS: &[&str] = &[
    "routing",
    "discussion",
    "recovery",
    "outcome",
    "review_policy",
    "completion",
    "memory_triage",
    "other",
];
fn normalized<'a>(value: &str, allowed: &[&'a str]) -> &'a str {
    allowed
        .iter()
        .copied()
        .find(|v| *v == value)
        .unwrap_or("other")
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
async fn read_only(path: &Path) -> Result<SqliteConnection> {
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .create_if_missing(false)
            .busy_timeout(Duration::from_millis(25)),
    )
    .await?;
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    db.lock_handle()
        .await?
        .set_progress_handler(1000, move || std::time::Instant::now() < deadline);
    sqlx::query("BEGIN").execute(&mut db).await?;
    Ok(db)
}
fn gauge(text: &mut String, name: &str, help: &str) {
    writeln!(text, "# HELP {name} {help}\n# TYPE {name} gauge").unwrap();
}
fn counter(text: &mut String, name: &str, help: &str) {
    let name = name.strip_suffix("_total").unwrap_or(name);
    writeln!(text, "# HELP {name} {help}\n# TYPE {name} counter").unwrap();
}

// Fold case after grouping so SQLite does not call lower() for every stored row.
// ASCII folding matches SQLite; merge differently cased and unknown groups.
async fn entity_groups(
    db: &mut SqliteConnection,
    table: &str,
    at: i64,
) -> Result<BTreeMap<&'static str, (i64, i64)>> {
    let rows=sqlx::query(&format!("SELECT CAST(json_extract(data,'$.status') AS TEXT) AS state,count(*) AS count,min(COALESCE(json_extract(data,'$.updated_at'),json_extract(data,'$.created_at'),?)) AS oldest FROM {table} GROUP BY json_extract(data,'$.status')")).bind(at).fetch_all(db).await?;
    let mut groups: BTreeMap<&str, (i64, i64)> =
        STATES.iter().map(|state| (*state, (0, at))).collect();
    for row in rows {
        let state: Option<String> = row.try_get("state")?;
        let state = state.as_deref().unwrap_or("other").to_ascii_lowercase();
        let state = normalized(&state, STATES);
        let group = groups.get_mut(state).unwrap();
        group.0 += row.try_get::<i64, _>("count")?;
        group.1 = group.1.min(row.try_get("oldest")?);
    }
    Ok(groups)
}

#[derive(Clone, Default)]
struct JevTotals {
    decisions: BTreeMap<(&'static str, &'static str), u64>,
    duration_buckets: BTreeMap<&'static str, [u64; JEV_DURATION_BUCKETS.len()]>,
    duration: BTreeMap<&'static str, (u64, f64)>,
    reasons: BTreeMap<(&'static str, &'static str), u64>,
    confidence: BTreeMap<&'static str, (u64, f64)>,
}
pub(super) struct Collector {
    tasks: PathBuf,
    memory: PathBuf,
    jev: PathBuf,
    metrics: Metrics,
    task_cursor: Option<i64>,
    task_identity: Option<String>,
    task_events: BTreeMap<&'static str, u64>,
    jev_cursor: Option<(i64, String)>,
    decision_totals: BTreeMap<(String, String), JevTotals>,
    decision_groups: BTreeSet<(String, String)>,
}
impl Collector {
    pub(super) fn new(tasks: PathBuf, memory: PathBuf, jev: PathBuf, metrics: Metrics) -> Self {
        Self {
            tasks,
            memory,
            jev,
            metrics,
            task_cursor: None,
            task_identity: None,
            task_events: BTreeMap::new(),
            jev_cursor: None,
            decision_totals: BTreeMap::new(),
            decision_groups: BTreeSet::new(),
        }
    }
    pub(super) async fn run(mut self, memory_enabled: bool) {
        let mut interval = tokio::time::interval(Duration::from_mins(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            for source in 0..3 {
                if source == 1 && !memory_enabled {
                    continue;
                }
                let result = tokio::time::timeout(Duration::from_secs(1), async {
                    match source {
                        0 => self.refresh_tasks().await,
                        1 => self.refresh_memory().await,
                        _ => self.refresh_jev().await,
                    }
                })
                .await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(source,error=%error,"application metrics snapshot unavailable");
                    }
                    Err(_) => {
                        tracing::warn!(source, "application metrics snapshot budget exhausted");
                    }
                }
            }
        }
    }
    // Keep each read transaction and its atomic publication together.
    #[allow(clippy::too_many_lines)]
    async fn refresh_tasks(&mut self) -> Result<()> {
        let mut db = read_only(&self.tasks).await?;
        let mut text = String::new();
        gauge(
            &mut text,
            "taskix_entities",
            "Stored workflow entities by status",
        );
        gauge(
            &mut text,
            "taskix_oldest_age_seconds",
            "Age since the oldest nonterminal entity update",
        );
        for (table, kind) in [("tasks", "task"), ("jobs", "job"), ("issues", "issue")] {
            let groups = entity_groups(&mut db, table, now()).await?;
            for (state, (count, oldest)) in groups {
                writeln!(
                    text,
                    "taskix_entities{{kind=\"{kind}\",state=\"{state}\"}} {count}"
                )?;
                if !matches!(state, "done" | "completed" | "cancelled" | "failed") {
                    let age = if count == 0 {
                        0
                    } else {
                        (now() - oldest).max(0)
                    };
                    writeln!(
                        text,
                        "taskix_oldest_age_seconds{{kind=\"{kind}\",state=\"{state}\"}} {age}"
                    )?;
                }
            }
        }
        let expired: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM task_leases WHERE json_extract(data,'$.lease_expires_at')<=?",
        )
        .bind(now())
        .fetch_one(&mut db)
        .await?;
        gauge(
            &mut text,
            "taskix_expired_leases",
            "Expired task execution leases",
        );
        writeln!(text, "taskix_expired_leases {expired}")?;
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM pending_documents")
            .fetch_one(&mut db)
            .await?;
        gauge(
            &mut text,
            "taskix_pending_documents",
            "Workflow documents awaiting projection",
        );
        writeln!(text, "taskix_pending_documents {pending}")?;
        let max: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0) FROM task_events")
            .fetch_one(&mut db)
            .await?;
        let pruned: i64 =
            sqlx::query_scalar("SELECT pruned_through FROM event_retention WHERE id=1")
                .fetch_one(&mut db)
                .await?;
        let identity: String =
            sqlx::query_scalar("SELECT instance_id FROM memory_source_identity WHERE singleton=1")
                .fetch_one(&mut db)
                .await?;
        let changed = self
            .task_identity
            .as_ref()
            .is_some_and(|previous| previous != &identity);
        self.task_identity = Some(identity);
        let head = max.max(pruned);
        let mut cursor = self.task_cursor.unwrap_or(head);
        if changed || head < cursor || pruned > cursor {
            self.task_cursor = Some(head);
            self.task_events.clear();
            self.metrics.clear(Snapshot::Tasks);
            anyhow::bail!("workflow event history changed; rebaselined without replay");
        }
        let rows=sqlx::query("SELECT sequence,json_extract(data,'$.event_type') AS event FROM task_events WHERE sequence>? ORDER BY sequence LIMIT ?").bind(cursor).bind(BATCH).fetch_all(&mut db).await?;
        let mut events = self.task_events.clone();
        for row in rows {
            cursor = row.try_get("sequence")?;
            let event: String = row.try_get("event")?;
            let event = normalized(
                &event,
                &[
                    "job.create",
                    "job.followup",
                    "job.submit",
                    "job.approve",
                    "job.cancel",
                    "job.reject",
                    "task.add",
                    "task.claim",
                    "task.start",
                    "task.done",
                    "task.wait",
                    "task.block",
                    "task.fail",
                    "task.cancel",
                    "task.retry",
                    "task.reopen",
                    "issue.create",
                    "issue.submit",
                    "issue.approve",
                    "issue.cancel",
                    "other",
                ],
            );
            *events.entry(event).or_default() += 1;
        }
        if !events.is_empty() {
            counter(
                &mut text,
                "taskix_workflow_events_total",
                "Workflow events observed since exporter baseline",
            );
            for (event, count) in &events {
                writeln!(
                    text,
                    "taskix_workflow_events_total{{event=\"{event}\"}} {count}"
                )?;
            }
        }
        self.task_cursor = Some(cursor);
        self.task_events = events;
        self.metrics.publish(Snapshot::Tasks, text);
        Ok(())
    }
    // Keep each read transaction and its atomic publication together.
    #[allow(clippy::too_many_lines)]
    async fn refresh_memory(&self) -> Result<()> {
        let mut db = read_only(&self.memory).await?;
        let mut text = String::new();
        gauge(
            &mut text,
            "taskix_memory_work_items",
            "Durable Memory work items by kind and state",
        );
        gauge(
            &mut text,
            "taskix_memory_oldest_pending_age_seconds",
            "Age of ready pending Memory work, excluding future retry delays",
        );
        let rows=sqlx::query("SELECT kind,state,count(*) AS count,min(CASE WHEN available_at<=? THEN available_at END) AS oldest FROM work_items GROUP BY kind,state").bind(now()).fetch_all(&mut db).await?;
        let mut groups = BTreeMap::new();
        for kind in ["extract", "consolidate", "other"] {
            for state in ["pending", "running", "done", "failed", "cancelled", "other"] {
                groups.insert((kind, state), (0_i64, now()));
            }
        }
        for row in rows {
            let raw_kind: String = row.try_get("kind")?;
            let kind = normalized(&raw_kind, &["extract", "consolidate", "other"]);
            let raw_state: String = row.try_get("state")?;
            let state = normalized(
                &raw_state,
                &["pending", "running", "done", "failed", "cancelled", "other"],
            );
            let group = groups.get_mut(&(kind, state)).unwrap();
            group.0 += row.try_get::<i64, _>("count")?;
            group.1 = group
                .1
                .min(row.try_get::<Option<i64>, _>("oldest")?.unwrap_or_else(now));
        }
        for ((kind, state), (count, oldest)) in groups {
            writeln!(
                text,
                "taskix_memory_work_items{{kind=\"{kind}\",state=\"{state}\"}} {count}"
            )?;
            if state == "pending" {
                let age = (now() - oldest).max(0);
                writeln!(
                    text,
                    "taskix_memory_oldest_pending_age_seconds{{kind=\"{kind}\"}} {age}"
                )?;
            }
        }
        for (name, query, help) in [
            (
                "taskix_memory_pending_embeddings",
                "SELECT count(*) FROM memories m JOIN embedding_profiles p ON p.project_id=m.project_id LEFT JOIN memory_vectors v ON v.memory_id=m.id AND v.generation=p.generation AND v.revision=m.revision WHERE m.status='active' AND (m.valid_until IS NULL OR m.valid_until>unixepoch()) AND v.memory_id IS NULL",
                "Active memories awaiting the configured embedding generation",
            ),
            (
                "taskix_memory_retrying_items",
                "SELECT count(*) FROM work_items WHERE state='pending' AND attempts>0",
                "Memory items waiting for a retry",
            ),
            (
                "taskix_memory_failed_items",
                "SELECT count(*) FROM work_items WHERE state='failed'",
                "Failed Memory work items",
            ),
            (
                "taskix_memory_pending_projection",
                "SELECT count(*) FROM memories m LEFT JOIN memory_projection p ON p.memory_id=m.id WHERE COALESCE(p.published_revision,0)<m.revision",
                "Memory documents awaiting projection",
            ),
            (
                "taskix_memory_embedding_failures",
                "SELECT count(*) FROM embedding_failures",
                "Memory embedding failures awaiting repair",
            ),
        ] {
            let count: i64 = sqlx::query_scalar(query).fetch_one(&mut db).await?;
            gauge(&mut text, name, help);
            writeln!(text, "{name} {count}")?;
        }
        gauge(&mut text, "taskix_memories", "Stored memories by status");
        let rows = sqlx::query("SELECT status,count(*) AS count FROM memories GROUP BY status")
            .fetch_all(&mut db)
            .await?;
        let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
        for row in rows {
            let status: String = row.try_get("status")?;
            *counts
                .entry(normalized(
                    &status.to_lowercase(),
                    &[
                        "active",
                        "superseded",
                        "forgotten",
                        "expired",
                        "candidate",
                        "rejected",
                        "other",
                    ],
                ))
                .or_default() += row.try_get::<i64, _>("count")?;
        }
        for (state, count) in counts {
            writeln!(text, "taskix_memories{{state=\"{state}\"}} {count}")?;
        }
        self.metrics.publish(Snapshot::Memory, text);
        Ok(())
    }
    // Keep each read transaction and its atomic publication together.
    #[allow(clippy::too_many_lines)]
    async fn refresh_jev(&mut self) -> Result<()> {
        let mut db = read_only(&self.jev).await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut db)
            .await?;
        let app: i64 = sqlx::query_scalar("PRAGMA application_id")
            .fetch_one(&mut db)
            .await?;
        ensure!(
            matches!(version, 1..=3) && app == 0x544A_4556,
            "unsupported Jev metrics schema"
        );
        let max: Option<(i64, String)> =
            sqlx::query_as("SELECT rowid,id FROM requests ORDER BY rowid DESC LIMIT 1")
                .fetch_optional(&mut db)
                .await?;
        let mut cursor = self
            .jev_cursor
            .clone()
            .unwrap_or_else(|| max.clone().unwrap_or((0, String::new())));
        if cursor.0 > 0 {
            let id: Option<String> = sqlx::query_scalar("SELECT id FROM requests WHERE rowid=?")
                .bind(cursor.0)
                .fetch_optional(&mut db)
                .await?;
            if id.as_deref() != Some(cursor.1.as_str()) {
                self.jev_cursor = max;
                self.decision_totals.clear();
                self.decision_groups.clear();
                self.metrics.clear(Snapshot::Jev);
                anyhow::bail!("Jev metrics database changed; rebaselined without replay");
            }
        }
        let kind = if version == 1 { "'routing'" } else { "r.kind" };
        let provider = if version < 3 {
            "'typesafe'"
        } else {
            "r.provider"
        };
        let protocol = if version < 3 {
            "'typesafe'"
        } else {
            "r.protocol"
        };
        let rows=sqlx::query(&format!("SELECT r.rowid,r.id,{kind} AS kind,{provider} AS provider,{protocol} AS protocol,r.accepted,r.reason,r.duration_ms,(SELECT avg(confidence) FROM answers a WHERE a.request_id=r.id) AS confidence FROM requests r WHERE r.rowid>? ORDER BY r.rowid LIMIT ?")).bind(cursor.0).bind(BATCH).fetch_all(&mut db).await?;
        let mut all_totals = self.decision_totals.clone();
        let mut groups = self.decision_groups.clone();
        for row in rows {
            cursor = (row.try_get("rowid")?, row.try_get("id")?);
            let group = decision_group(
                row.try_get("provider")?,
                row.try_get("protocol")?,
                &mut groups,
            );
            let totals = all_totals.entry(group).or_default();
            let raw: String = row.try_get("kind")?;
            let kind = normalized(&raw, KINDS);
            let accepted: i64 = row.try_get("accepted")?;
            *totals
                .decisions
                .entry((
                    kind,
                    if accepted == 1 {
                        "accepted"
                    } else {
                        "fallback"
                    },
                ))
                .or_default() += 1;
            let duration: f64 = row.try_get("duration_ms")?;
            if duration.is_finite() && duration >= 0.0 {
                let group = totals.duration.entry(kind).or_default();
                group.0 += 1;
                let seconds = duration / 1000.0;
                group.1 += seconds;
                let buckets = totals.duration_buckets.entry(kind).or_default();
                for (bound, count) in JEV_DURATION_BUCKETS.iter().zip(buckets) {
                    if seconds <= *bound {
                        *count += 1;
                    }
                }
            }
            if let Some(confidence) = row.try_get::<Option<f64>, _>("confidence")?
                && confidence.is_finite()
                && (0.0..=1.0).contains(&confidence)
            {
                let group = totals.confidence.entry(kind).or_default();
                group.0 += 1;
                group.1 += confidence;
            }
            if accepted != 1 {
                let reason: Option<String> = row.try_get("reason")?;
                let reason = normalized(
                    reason.as_deref().unwrap_or("other"),
                    &[
                        "low_confidence",
                        "uncertain",
                        "invalid_response",
                        "timeout",
                        "provider_error",
                        "disabled",
                        "preparation_unavailable",
                        "other",
                    ],
                );
                *totals.reasons.entry((kind, reason)).or_default() += 1;
            }
        }
        let mut text = String::new();
        let mut declarations = BTreeSet::new();
        for ((provider, protocol), totals) in &all_totals {
            let labels = format!("provider=\"{provider}\",protocol=\"{protocol}\",");
            render_decision_totals(
                &mut text,
                totals,
                "taskix_decision",
                &labels,
                &mut declarations,
            )?;
            if provider == "typesafe" && protocol == "typesafe" {
                render_decision_totals(&mut text, totals, "taskix_jev", "", &mut declarations)?;
            }
        }
        let rows=sqlx::query(&format!("SELECT {provider} AS provider,{protocol} AS protocol,{kind} AS kind,review,count(*) AS count FROM requests r WHERE review IS NOT NULL GROUP BY 1,2,3,4")).fetch_all(&mut db).await?;
        let mut reviews = BTreeMap::new();
        for row in rows {
            let (provider, protocol) = decision_group(
                row.try_get("provider")?,
                row.try_get("protocol")?,
                &mut groups,
            );
            let raw: String = row.try_get("kind")?;
            let review: String = row.try_get("review")?;
            *reviews
                .entry((
                    provider,
                    protocol,
                    normalized(&raw, KINDS),
                    normalized(&review, &["correct", "incorrect", "other"]),
                ))
                .or_insert(0i64) += row.try_get::<i64, _>("count")?;
        }
        for ((provider, protocol, kind, result), count) in reviews {
            declare_decision(
                &mut text,
                &mut declarations,
                "taskix_decision_reviewed",
                "gauge",
            )?;
            writeln!(
                text,
                "taskix_decision_reviewed{{provider=\"{provider}\",protocol=\"{protocol}\",kind=\"{kind}\",result=\"{result}\"}} {count}"
            )?;
            if provider == "typesafe" && protocol == "typesafe" {
                declare_decision(&mut text, &mut declarations, "taskix_jev_reviewed", "gauge")?;
                writeln!(
                    text,
                    "taskix_jev_reviewed{{kind=\"{kind}\",result=\"{result}\"}} {count}"
                )?;
            }
        }
        self.jev_cursor = Some(cursor);
        self.decision_totals = all_totals;
        self.decision_groups = groups;
        self.metrics.publish(Snapshot::Jev, text);
        Ok(())
    }
}
// Provider identities come from configuration, never request content. Bound retained
// identities even if a shared historical database contains many configurations.
const MAX_DECISION_GROUPS: usize = 32;
fn decision_group(
    provider: &str,
    protocol: &str,
    groups: &mut BTreeSet<(String, String)>,
) -> (String, String) {
    if !crate::decision::valid_provider(provider)
        || !matches!(protocol, "typesafe" | "openai_decisions")
    {
        return ("other".into(), "other".into());
    }
    let group = (provider.to_owned(), protocol.to_owned());
    if matches!(provider, "typesafe" | "openai") {
        return group;
    }
    if groups.contains(&group) || groups.len() < MAX_DECISION_GROUPS {
        groups.insert(group.clone());
        group
    } else {
        ("other".into(), "other".into())
    }
}
fn declare_decision(
    text: &mut String,
    declarations: &mut BTreeSet<String>,
    name: &str,
    kind: &str,
) -> Result<()> {
    if declarations.insert(name.to_owned()) {
        writeln!(
            text,
            "# HELP {name} Persisted decision observations; adoption is not accuracy\n# TYPE {name} {kind}"
        )?;
    }
    Ok(())
}
fn render_decision_totals(
    text: &mut String,
    totals: &JevTotals,
    prefix: &str,
    labels: &str,
    declarations: &mut BTreeSet<String>,
) -> Result<()> {
    if !totals.decisions.is_empty() {
        declare_decision(
            text,
            declarations,
            &format!("{prefix}_decisions_total"),
            "counter",
        )?;
        for ((kind, outcome), count) in &totals.decisions {
            writeln!(
                text,
                "{prefix}_decisions_total{{{labels}kind=\"{kind}\",outcome=\"{outcome}\"}} {count}"
            )?;
        }
    }
    for (suffix, groups) in [
        ("duration_seconds", &totals.duration),
        ("request_confidence", &totals.confidence),
    ] {
        if groups.is_empty() {
            continue;
        }
        let name = format!("{prefix}_{suffix}");
        let histogram = suffix == "duration_seconds";
        declare_decision(
            text,
            declarations,
            &name,
            if histogram { "histogram" } else { "summary" },
        )?;
        for (kind, (count, sum)) in groups {
            if histogram {
                for (bound, value) in JEV_DURATION_BUCKETS
                    .iter()
                    .zip(&totals.duration_buckets[kind])
                {
                    writeln!(
                        text,
                        "{name}_bucket{{{labels}kind=\"{kind}\",le=\"{bound}\"}} {value}"
                    )?;
                }
                writeln!(
                    text,
                    "{name}_bucket{{{labels}kind=\"{kind}\",le=\"+Inf\"}} {count}"
                )?;
            }
            writeln!(
                text,
                "{name}_sum{{{labels}kind=\"{kind}\"}} {sum}\n{name}_count{{{labels}kind=\"{kind}\"}} {count}"
            )?;
        }
    }
    if !totals.reasons.is_empty() {
        declare_decision(
            text,
            declarations,
            &format!("{prefix}_fallbacks_total"),
            "counter",
        )?;
        for ((kind, reason), count) in &totals.reasons {
            writeln!(
                text,
                "{prefix}_fallbacks_total{{{labels}kind=\"{kind}\",reason=\"{reason}\"}} {count}"
            )?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;

pub(super) fn load_config(path: &Path) -> Result<agentix_metrics::Config> {
    let document: toml::Value = toml::from_str(&std::fs::read_to_string(path)?)?;
    let Some(mut value) = document.get("metrics").cloned() else {
        return Ok(agentix_metrics::Config::taskix());
    };
    if let Some(table) = value.as_table_mut() {
        table
            .entry("listen")
            .or_insert_with(|| toml::Value::String("127.0.0.1:9465".into()));
    }
    let config: agentix_metrics::Config = value.try_into()?;
    config.validate()?;
    Ok(config)
}
pub(super) struct Runtime {
    _server: agentix_metrics::Exporter,
    collector: tokio::task::JoinHandle<()>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.collector.abort();
    }
}
pub(super) async fn start(
    config: &agentix_metrics::Config,
    location: &agentix_memory::MemoryLocation,
) -> Result<Option<Runtime>> {
    if !config.enabled {
        return Ok(None);
    }
    let metrics = Metrics::new(
        agentix_metrics::Service::Taskix,
        env!("AGENTIX_BUILD_VERSION"),
    );
    let server = metrics.serve(config.listen).await?;
    let collector = Collector::new(
        location.task_path.clone(),
        location.path.clone(),
        crate::metrics::collector::database_path()?,
        metrics.clone(),
    );
    agentix_metrics::install(metrics)?;
    let collector = tokio::spawn(collector.run(location.enabled));
    Ok(Some(Runtime {
        _server: server,
        collector,
    }))
}
