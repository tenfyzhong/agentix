use std::{path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use sqlx::{
    SqliteConnection, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use crate::{Actor, Kind, Memory, MemoryInput, Source, Status, retrieval};

#[derive(Debug, serde::Serialize)]
pub struct SourceSummary {
    pub receipt_id: String,
    pub turn_id: String,
    pub revision: i64,
    pub recorded_at: i64,
    pub message_count: i64,
}

#[derive(Clone)]
pub struct MemoryStore {
    pub(crate) pool: SqlitePool,
    changes: tokio::sync::broadcast::Sender<String>,
    work_changes: tokio::sync::watch::Sender<u64>,
}

impl MemoryStore {
    #[must_use]
    pub fn subscribe_work(&self) -> tokio::sync::watch::Receiver<u64> {
        self.work_changes.subscribe()
    }

    pub(crate) fn notify_work(&self) {
        self.work_changes
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    #[must_use]
    pub fn subscribe_changes(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.changes.subscribe()
    }

    pub(crate) fn notify_change(&self, project: &str) {
        let _ = self.changes.send(project.to_owned());
    }

    /// Offline fallback opens an existing database without creating or migrating it.
    pub async fn open_read_only(path: &Path) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .read_only(true)
                    .foreign_keys(true)
                    .busy_timeout(Duration::from_secs(2)),
            )
            .await?;
        let app: i64 = sqlx::query_scalar("PRAGMA application_id")
            .fetch_one(&pool)
            .await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&pool)
            .await?;
        ensure!(
            app == 0x4158_4d4d && version == 1,
            "unsupported memory database identity or schema"
        );
        Ok(Self {
            pool,
            changes: tokio::sync::broadcast::channel(256).0,
            work_changes: tokio::sync::watch::channel(0).0,
        })
    }

    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let app: i64 = sqlx::query_scalar("PRAGMA application_id")
            .fetch_one(&mut *tx)
            .await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'")
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            app == 0x4158_4d4d || (app == 0 && count == 0),
            "invalid: dedicated memory database required"
        );
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut *tx)
            .await?;
        ensure!(version <= 1, "unsupported memory database schema {version}");
        sqlx::raw_sql(include_str!("schema.sql"))
            .execute(&mut *tx)
            .await?;
        let indexed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM memory_metadata WHERE key='source_turn_index_version')",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !indexed {
            sqlx::query("INSERT OR REPLACE INTO source_turns(project_id,instance_id,session_id,turn_id,first_sequence,receipt_id,revision,recorded_at,message_count) SELECT h.project_id,h.instance_id,h.session_id,h.turn_id,history.first_sequence,h.receipt_id,h.revision,json_extract(current.data,'$.recorded_at'),json_array_length(current.data,'$.messages') FROM (SELECT project_id,instance_id,json_extract(data,'$.session_id') session_id,json_extract(data,'$.turn_id') turn_id,min(json_extract(data,'$.sequence')) first_sequence FROM sources GROUP BY project_id,instance_id,json_extract(data,'$.session_id'),json_extract(data,'$.turn_id')) history JOIN source_heads h ON h.project_id=history.project_id AND h.instance_id=history.instance_id AND h.session_id=history.session_id AND h.turn_id=history.turn_id JOIN sources current ON current.receipt_id=h.receipt_id")
                .execute(&mut *tx).await?;
            sqlx::query(
                "INSERT INTO memory_metadata(key,value) VALUES('source_turn_index_version','1')",
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Self {
            pool,
            changes: tokio::sync::broadcast::channel(256).0,
            work_changes: tokio::sync::watch::channel(0).0,
        })
    }

    /// Bind once to a source database and resume its ordered replay checkpoint.
    /// Older databases have no checkpoint and replay from zero to repair possible gaps.
    pub async fn bind_source(&self, instance: &str) -> Result<i64> {
        ensure!(!instance.is_empty(), "invalid: source instance");
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT OR IGNORE INTO memory_metadata(key,value) VALUES ('source_instance',?)",
        )
        .bind(instance)
        .execute(&mut *tx)
        .await?;
        let bound: String =
            sqlx::query_scalar("SELECT value FROM memory_metadata WHERE key='source_instance'")
                .fetch_one(&mut *tx)
                .await?;
        let foreign: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sources WHERE instance_id<>?)")
                .bind(instance)
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            bound == instance && !foreign,
            "conflict: memory belongs to another task database; restore a compatible pair"
        );
        sqlx::query(
            "INSERT OR IGNORE INTO memory_metadata(key,value) VALUES ('replay_cursor','0')",
        )
        .execute(&mut *tx)
        .await?;
        let cursor: String =
            sqlx::query_scalar("SELECT value FROM memory_metadata WHERE key='replay_cursor'")
                .fetch_one(&mut *tx)
                .await?;
        let cursor: i64 = cursor.parse().context("invalid replay checkpoint")?;
        ensure!(cursor >= 0, "invalid replay checkpoint");
        tx.commit().await?;
        Ok(cursor)
    }

    /// Advance after the next source in task-outbox order has been committed.
    /// Intake may commit later sources independently; it must never advance this cursor.
    /// A crash before this transaction only repeats the idempotent ingest.
    pub async fn checkpoint_replay(&self, after: i64, source: &Source) -> Result<()> {
        ensure!(
            after >= 0 && source.sequence > after,
            "invalid replay progress"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let data: Option<String> =
            sqlx::query_scalar("SELECT data FROM sources WHERE receipt_id=?")
                .bind(&source.receipt_id)
                .fetch_optional(&mut *tx)
                .await?;
        ensure!(
            data.as_deref() == Some(serde_json::to_string(source)?.as_str()),
            "replay source must be persisted before checkpointing"
        );
        let result =
            sqlx::query("UPDATE memory_metadata SET value=? WHERE key='replay_cursor' AND value=?")
                .bind(source.sequence.to_string())
                .bind(after.to_string())
                .execute(&mut *tx)
                .await?;
        ensure!(
            result.rows_affected() == 1,
            "conflict: replay checkpoint changed or source is unbound"
        );
        tx.commit().await?;
        Ok(())
    }

    pub async fn recovery_sources(&self, after: &str, limit: i64) -> Result<Vec<Source>> {
        ensure!((1..=100).contains(&limit), "invalid: recovery page");
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT data FROM sources WHERE receipt_id>? ORDER BY receipt_id LIMIT ?",
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|data| Ok(serde_json::from_str(&data)?))
            .collect()
    }

    pub async fn ingest(&self, source: &Source) -> Result<bool> {
        ensure!(
            !source.receipt_id.is_empty() && !source.project_id.is_empty() && source.revision > 0,
            "invalid: source identity"
        );
        ensure!(
            source
                .messages
                .iter()
                .all(|m| !m.id.is_empty() && matches!(m.role.as_str(), "user" | "assistant")),
            "invalid: source messages"
        );
        let data = serde_json::to_string(source)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound: Option<String> =
            sqlx::query_scalar("SELECT value FROM memory_metadata WHERE key='source_instance'")
                .fetch_optional(&mut *tx)
                .await?;
        ensure!(
            bound
                .as_deref()
                .is_none_or(|value| value == source.instance_id),
            "conflict: foreign source instance"
        );
        let existing: Option<String> =
            sqlx::query_scalar("SELECT data FROM sources WHERE receipt_id=?")
                .bind(&source.receipt_id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(existing) = existing {
            ensure!(
                existing == data,
                "conflict: immutable source receipt changed"
            );
            return Ok(false);
        }
        sqlx::query("INSERT INTO sources(receipt_id,project_id,instance_id,data) VALUES (?,?,?,?)")
            .bind(&source.receipt_id)
            .bind(&source.project_id)
            .bind(&source.instance_id)
            .bind(data)
            .execute(&mut *tx)
            .await?;
        crate::queue::enqueue_source(&mut tx, source).await?;
        sqlx::query("INSERT INTO source_turns(project_id,instance_id,session_id,turn_id,first_sequence,receipt_id,revision,recorded_at,message_count) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(instance_id,session_id,turn_id) DO UPDATE SET first_sequence=min(first_sequence,excluded.first_sequence),receipt_id=CASE WHEN excluded.revision>revision THEN excluded.receipt_id ELSE receipt_id END,recorded_at=CASE WHEN excluded.revision>revision THEN excluded.recorded_at ELSE recorded_at END,message_count=CASE WHEN excluded.revision>revision THEN excluded.message_count ELSE message_count END,revision=max(revision,excluded.revision)")
            .bind(&source.project_id).bind(&source.instance_id).bind(&source.session_id).bind(&source.turn_id).bind(source.sequence).bind(&source.receipt_id).bind(source.revision).bind(source.recorded_at).bind(i64::try_from(source.messages.len())?).execute(&mut *tx).await?;
        tx.commit().await?;
        self.notify_work();
        Ok(true)
    }

    pub async fn source(&self, project: &str, receipt: &str) -> Result<Source> {
        let data: String =
            sqlx::query_scalar("SELECT data FROM sources WHERE project_id=? AND receipt_id=?")
                .bind(project)
                .bind(receipt)
                .fetch_optional(&self.pool)
                .await?
                .context("not_found: memory source")?;
        Ok(serde_json::from_str(&data)?)
    }

    /// Walk preceding receipts in the anchor's conversation, never another session
    /// or Project. Turn order follows first receipt sequence, so later attachment or
    /// edits do not move a prior turn past the anchor. Return current revisions.
    pub async fn source_neighbors(
        &self,
        project: &str,
        receipt: &str,
    ) -> Result<Vec<SourceSummary>> {
        let rows = sqlx::query_as::<_, (String,String,i64,i64,i64)>("SELECT t.receipt_id,t.turn_id,t.revision,t.recorded_at,t.message_count FROM sources anchor JOIN source_turns a ON a.project_id=anchor.project_id AND a.instance_id=anchor.instance_id AND a.session_id=json_extract(anchor.data,'$.session_id') AND a.turn_id=json_extract(anchor.data,'$.turn_id') JOIN source_turns t ON t.project_id=a.project_id AND t.instance_id=a.instance_id AND t.session_id=a.session_id AND t.first_sequence<a.first_sequence WHERE anchor.project_id=? AND anchor.receipt_id=? ORDER BY t.first_sequence DESC LIMIT 8")
            .bind(project).bind(receipt).fetch_all(&self.pool).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sources WHERE project_id=? AND receipt_id=?)",
        )
        .bind(project)
        .bind(receipt)
        .fetch_one(&self.pool)
        .await?;
        ensure!(exists, "not_found: memory source");
        Ok(rows
            .into_iter()
            .map(
                |(receipt_id, turn_id, revision, recorded_at, message_count)| SourceSummary {
                    receipt_id,
                    turn_id,
                    revision,
                    recorded_at,
                    message_count,
                },
            )
            .collect())
    }

    pub async fn create(
        &self,
        project: &str,
        content: MemoryInput,
        actor: Actor,
    ) -> Result<Memory> {
        content.validate(actor)?;
        ensure!(!project.is_empty(), "invalid: memory Project");
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_evidence(&mut tx, project, &content).await?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let memory = Memory {
            id: format!("mem_{}", uuid::Uuid::now_v7().simple()),
            project_id: project.into(),
            revision: 1,
            status: Status::Active,
            actor,
            created_at: now,
            updated_at: now,
            reason: String::new(),
            supersedes: None,
            superseded_by: None,
            content,
        };
        save(&mut tx, &memory).await?;
        tx.commit().await?;
        self.notify_change(project);
        Ok(memory)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn supersede(
        &self,
        project: &str,
        id: &str,
        revision: i64,
        content: MemoryInput,
        reason: &str,
        actor: Actor,
    ) -> Result<Memory> {
        content.validate(actor)?;
        ensure!(
            !reason.trim().is_empty() && reason.len() <= 2048,
            "invalid: supersession reason"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut prior = load(&mut tx, project, id).await?;
        ensure!(
            prior.revision == revision && prior.status.searchable(),
            "conflict: superseded memory changed"
        );
        validate_evidence(&mut tx, project, &content).await?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let memory = Memory {
            id: format!("mem_{}", uuid::Uuid::now_v7().simple()),
            project_id: project.into(),
            revision: 1,
            status: Status::Active,
            actor,
            created_at: now,
            updated_at: now,
            reason: reason.into(),
            supersedes: Some(id.into()),
            superseded_by: None,
            content,
        };
        prior.revision += 1;
        prior.status = Status::Superseded;
        prior.superseded_by = Some(memory.id.clone());
        prior.updated_at = now;
        prior.reason = reason.into();
        prior.actor = actor;
        save(&mut tx, &memory).await?;
        save(&mut tx, &prior).await?;
        tx.commit().await?;
        self.notify_change(project);
        Ok(memory)
    }

    pub async fn update(
        &self,
        project: &str,
        id: &str,
        revision: i64,
        content: MemoryInput,
        actor: Actor,
    ) -> Result<Memory> {
        content.validate(actor)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut memory = load(&mut tx, project, id).await?;
        ensure!(
            memory.revision == revision,
            "conflict: memory revision changed"
        );
        ensure!(
            memory.status != Status::Forgotten,
            "conflict: memory was forgotten"
        );
        validate_evidence(&mut tx, project, &content).await?;
        memory.revision += 1;
        memory.content = content;
        memory.actor = actor;
        memory.updated_at = time::OffsetDateTime::now_utc().unix_timestamp();
        save(&mut tx, &memory).await?;
        tx.commit().await?;
        self.notify_change(project);
        Ok(memory)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_status(
        &self,
        project: &str,
        id: &str,
        revision: i64,
        status: Status,
        reason: &str,
        actor: Actor,
    ) -> Result<Memory> {
        ensure!(
            !reason.trim().is_empty() && reason.len() <= 2048,
            "invalid: lifecycle reason required"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut memory = load(&mut tx, project, id).await?;
        ensure!(
            memory.revision == revision,
            "conflict: memory revision changed"
        );
        ensure!(
            memory.status != Status::Forgotten,
            "conflict: memory was forgotten"
        );
        if status == Status::Forgotten {
            // Merges and edits can replace evidence. Suppress every historical
            // source of this memory, including versions written by older builds.
            let versions: Vec<String> = sqlx::query_scalar(
                "SELECT data FROM memory_versions WHERE memory_id=? ORDER BY revision",
            )
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
            for data in versions {
                let version: Memory = serde_json::from_str(&data)?;
                for key in evidence_keys(&mut tx, project, &version.content).await? {
                    sqlx::query("INSERT OR IGNORE INTO suppressions(project_id,evidence_key,memory_id) VALUES (?,?,?)")
                        .bind(project).bind(key).bind(id).execute(&mut *tx).await?;
                }
            }
        }
        memory.revision += 1;
        memory.status = status;
        memory.reason = reason.into();
        memory.actor = actor;
        memory.updated_at = time::OffsetDateTime::now_utc().unix_timestamp();
        save(&mut tx, &memory).await?;
        tx.commit().await?;
        self.notify_change(project);
        Ok(memory)
    }

    pub async fn show(&self, project: &str, id: &str, revision: Option<i64>) -> Result<Memory> {
        let data: Option<String> = if let Some(revision) = revision {
            sqlx::query_scalar("SELECT v.data FROM memory_versions v JOIN memories m ON m.id=v.memory_id WHERE m.project_id=? AND m.id=? AND v.revision=?")
                .bind(project).bind(id).bind(revision).fetch_optional(&self.pool).await?
        } else {
            sqlx::query_scalar("SELECT data FROM memories WHERE project_id=? AND id=?")
                .bind(project)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
        };
        Ok(serde_json::from_str(&data.context("not_found: memory")?)?)
    }

    pub async fn search(&self, project: &str, query: &str, limit: i64) -> Result<Vec<Memory>> {
        ensure!((1..=100).contains(&limit), "invalid: memory search limit");
        let Some(query) = retrieval::query(project, query)? else {
            return Ok(Vec::new());
        };
        let rows:Vec<String>=sqlx::query_scalar("SELECT m.data FROM memory_fts JOIN memories m ON m.rowid=memory_fts.rowid WHERE memory_fts MATCH ? AND m.project_id=? AND m.status IN ('active','conflicted') AND (m.valid_until IS NULL OR m.valid_until>unixepoch()) ORDER BY bm25(memory_fts,0,5,1,4,2),m.id LIMIT ?")
            .bind(query).bind(project).bind(limit).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }

    pub async fn list(
        &self,
        project: &str,
        after: &str,
        limit: i64,
        include_inactive: bool,
    ) -> Result<Vec<Memory>> {
        ensure!((1..=100).contains(&limit), "invalid: memory page size");
        let rows:Vec<String>=sqlx::query_scalar("SELECT data FROM memories WHERE project_id=? AND id>? AND (? OR (status IN ('active','conflicted') AND (valid_until IS NULL OR valid_until>unixepoch()))) ORDER BY id LIMIT ?")
            .bind(project).bind(after).bind(include_inactive).bind(limit).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }

    pub async fn reindex_fts(
        &self,
        project: &str,
        after: &str,
        limit: i64,
    ) -> Result<crate::IndexPage> {
        ensure!((1..=100).contains(&limit), "invalid: reindex page size");
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT data FROM memories WHERE project_id=? AND id>? ORDER BY id LIMIT ?",
        )
        .bind(project)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        let mut page = crate::IndexPage {
            indexed: rows.len(),
            next_cursor: after.into(),
            complete: rows.len() < usize::try_from(limit)?,
        };
        for row in rows {
            let memory: Memory = serde_json::from_str(&row)?;
            page.next_cursor.clone_from(&memory.id);
            index_memory(&mut tx, &memory).await?;
            sqlx::query("DELETE FROM embedding_failures WHERE memory_id=?")
                .bind(&memory.id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.notify_change(project);
        Ok(page)
    }
}

pub(crate) async fn load(conn: &mut SqliteConnection, project: &str, id: &str) -> Result<Memory> {
    let data: String = sqlx::query_scalar("SELECT data FROM memories WHERE project_id=? AND id=?")
        .bind(project)
        .bind(id)
        .fetch_optional(conn)
        .await?
        .context("not_found: memory")?;
    Ok(serde_json::from_str(&data)?)
}

async fn evidence_keys(
    conn: &mut SqliteConnection,
    project: &str,
    content: &MemoryInput,
) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut user_evidence = false;
    for evidence in &content.evidence {
        let data: String =
            sqlx::query_scalar("SELECT data FROM sources WHERE receipt_id=? AND project_id=?")
                .bind(&evidence.receipt_id)
                .bind(project)
                .fetch_optional(&mut *conn)
                .await?
                .context("invalid: evidence outside Project or absent")?;
        let source: Source = serde_json::from_str(&data)?;
        let message = source
            .messages
            .iter()
            .find(|m| m.id == evidence.message_id)
            .context("invalid: evidence message absent")?;
        ensure!(
            message.text.contains(&evidence.quote),
            "invalid: evidence quote is not in source"
        );
        user_evidence |= message.role == "user";
        keys.push(retrieval::digest(&serde_json::to_string(&(
            &source.instance_id,
            &source.session_id,
            &message.id,
            &message.text,
        ))?));
    }
    ensure!(
        content.evidence.is_empty()
            || !matches!(content.kind, Kind::UserDecision | Kind::UserAssertion)
            || user_evidence,
        "invalid: assistant text alone cannot establish a user decision"
    );
    Ok(keys)
}

pub(crate) async fn validate_evidence(
    conn: &mut SqliteConnection,
    project: &str,
    content: &MemoryInput,
) -> Result<()> {
    for key in evidence_keys(conn, project, content).await? {
        let suppressed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM suppressions WHERE project_id=? AND evidence_key=?)",
        )
        .bind(project)
        .bind(key)
        .fetch_one(&mut *conn)
        .await?;
        ensure!(!suppressed, "conflict: evidence was forgotten");
    }
    Ok(())
}

pub(crate) async fn save(conn: &mut SqliteConnection, memory: &Memory) -> Result<()> {
    let data = serde_json::to_string(memory)?;
    let status = serde_json::to_value(memory.status)?
        .as_str()
        .context("invalid memory status")?
        .to_owned();
    sqlx::query("INSERT INTO memories(id,project_id,revision,status,data) VALUES (?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,status=excluded.status,data=excluded.data")
        .bind(&memory.id).bind(&memory.project_id).bind(memory.revision).bind(status).bind(&data).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO memory_versions(memory_id,revision,data) VALUES (?,?,?)")
        .bind(&memory.id)
        .bind(memory.revision)
        .bind(data)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM memory_vectors WHERE memory_id=?")
        .bind(&memory.id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM embedding_failures WHERE memory_id=?")
        .bind(&memory.id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT OR IGNORE INTO memory_projection(memory_id) VALUES(?)")
        .bind(&memory.id)
        .execute(&mut *conn)
        .await?;
    index_memory(conn, memory).await
}

async fn index_memory(conn: &mut SqliteConnection, memory: &Memory) -> Result<()> {
    let row: i64 = sqlx::query_scalar("SELECT rowid FROM memories WHERE id=?")
        .bind(&memory.id)
        .fetch_one(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM memory_fts WHERE rowid=?")
        .bind(row)
        .execute(&mut *conn)
        .await?;
    if memory.status.searchable() {
        sqlx::query("INSERT INTO memory_fts(rowid,project_token,title,body,tags,scope) VALUES (?,?,?,?,?,?)")
            .bind(row).bind(retrieval::project_token(&memory.project_id))
            .bind(retrieval::index_text(&memory.content.title))
            .bind(retrieval::index_text(&format!("{} {} {}",memory.content.conclusion,memory.content.rationale,memory.content.conditions.join(" "))))
            .bind(retrieval::index_text(&memory.content.tags.join(" ")))
            .bind(retrieval::index_text(&memory.content.scope)).execute(conn).await?;
    }
    Ok(())
}
