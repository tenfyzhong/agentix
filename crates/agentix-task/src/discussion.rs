//! Session drafts are separate from Jobs and never projected into Obsidian.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

use crate::{Snapshot, Store, WriteOptions, mutations::required};

const RETENTION: i64 = 30 * 24 * 60 * 60;

async fn expire(conn: &mut SqliteConnection, now: i64) -> Result<()> {
    sqlx::query("UPDATE discussion_sessions SET revision=revision+1 WHERE activity<=? AND session_id IN (SELECT session_id FROM discussion_turns WHERE job_id IS NULL)")
        .bind(now - RETENTION).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM discussion_turns WHERE job_id IS NULL AND session_id IN (SELECT session_id FROM discussion_sessions WHERE activity<=?)")
        .bind(now - RETENTION).execute(conn).await?;
    Ok(())
}

pub(crate) async fn prepare(
    conn: &mut SqliteConnection,
    request: &mut Value,
    options: &WriteOptions,
    now: i64,
) -> Result<()> {
    let command = required(request, "command")?;
    if command == "session.start" {
        expire(conn, now).await?;
    }
    if command != "session.record" || request.get("turn_id").is_none() {
        return Ok(());
    }
    let session = required(request, "session")?.to_owned();
    ensure!(
        options.session_ref.as_deref() == Some(&session),
        "conflict: conversation session mismatch"
    );
    let turn = required(request, "turn_id")?.to_owned();
    let incoming = request["messages"]
        .as_array()
        .context("invalid: conversation messages")?;
    expire(conn, now).await?;
    sqlx::query("INSERT INTO discussion_sessions(session_id,revision,activity) VALUES (?,0,?) ON CONFLICT(session_id) DO UPDATE SET activity=MAX(activity,excluded.activity)")
        .bind(&session).bind(now).execute(&mut *conn).await?;
    let existing = sqlx::query(
        "SELECT messages,job_id FROM discussion_turns WHERE session_id=? AND turn_id=?",
    )
    .bind(&session)
    .bind(&turn)
    .fetch_optional(&mut *conn)
    .await?;
    let mut messages: Vec<Value> = existing
        .as_ref()
        .map(|r| serde_json::from_str(&r.get::<String, _>("messages")))
        .transpose()?
        .unwrap_or_default();
    let original = messages.clone();
    for message in incoming {
        let id = required(message, "id")?;
        let role = required(message, "role")?;
        ensure!(
            matches!(role, "user" | "assistant"),
            "invalid: only user and assistant text may be recorded"
        );
        let text = required(message, "text")?;
        let text = if role == "user" {
            crate::conversation::user_text(text)
        } else {
            text
        };
        if text.trim().is_empty() {
            continue;
        }
        let value = json!({"id":id,"role":role,"text":text});
        if let Some(old) = messages.iter_mut().find(|m| m["id"] == id) {
            ensure!(old["role"] == role, "conflict: message role changed");
            *old = value;
        } else {
            messages.push(value);
        }
    }
    let bound: Option<String> = existing.as_ref().and_then(|r| r.get("job_id"));
    if original != messages || existing.is_none() {
        sqlx::query("INSERT INTO discussion_turns(session_id,turn_id,source,messages) VALUES (?,?,?,?) ON CONFLICT(session_id,turn_id) DO UPDATE SET messages=excluded.messages")
            .bind(&session).bind(&turn).bind(request["source"].as_str().unwrap_or("host")).bind(json!(messages).to_string()).execute(&mut *conn).await?;
        sqlx::query("UPDATE discussion_sessions SET revision=revision+1 WHERE session_id=?")
            .bind(&session)
            .execute(&mut *conn)
            .await?;
    }
    if let Some(job) = bound {
        if let Some(explicit) = request["job"].as_str() {
            ensure!(explicit == job, "conflict: turn belongs to another Job");
        }
        request["job"] = json!(job);
        request["messages"] = ordered_messages(conn, &session, &job).await?;
        request["ordered"] = json!(true);
    } else {
        // Association is an explicit lifecycle/attach decision, never latest-Job inference.
        ensure!(
            request["job"].is_null(),
            "conflict: bind the turn before recording into a Job"
        );
        request["command"] = json!("session.stage");
    }
    Ok(())
}

async fn ordered_messages(conn: &mut SqliteConnection, session: &str, job: &str) -> Result<Value> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT messages FROM discussion_turns WHERE session_id=? AND job_id=? ORDER BY position",
    )
    .bind(session)
    .bind(job)
    .fetch_all(conn)
    .await?;
    let mut messages = Vec::<Value>::new();
    for row in rows {
        messages.extend(serde_json::from_str::<Vec<Value>>(&row)?);
    }
    Ok(json!(messages))
}

fn validate_target(job: &crate::Job, request: &Value) -> Result<()> {
    if let Some(expected) = request["conversation_target"].as_str() {
        let target = json!([
            job.title,
            job.goal,
            request["prompt"].as_str().unwrap_or_default()
        ]);
        ensure!(
            crate::store::hash_bytes(target.to_string().as_bytes()) == expected,
            "conflict: discussion delivery target changed"
        );
    }
    Ok(())
}

pub(crate) async fn attach(
    conn: &mut SqliteConnection,
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    result: &mut Value,
    now: i64,
) -> Result<()> {
    let Some(turns) = request
        .get("conversation_turns")
        .and_then(Value::as_array)
        .filter(|t| !t.is_empty())
    else {
        return Ok(());
    };
    ensure!(
        matches!(
            request["command"].as_str(),
            Some("job.create" | "job.followup" | "conversation.attach")
        ),
        "invalid: conversation selection is not supported for this command"
    );
    let session = options
        .session_ref
        .as_deref()
        .context("invalid: discussion attachment requires session")?;
    let job_id = required(result, "id")?.to_owned();
    let index = state.job_index(&job_id)?;
    validate_target(&state.jobs[index], request)?;
    crate::conversation::session_job_index(state, &json!({"job":job_id}), session)?;
    expire(conn, now).await?;
    let revision: Option<i64> =
        sqlx::query_scalar("SELECT revision FROM discussion_sessions WHERE session_id=?")
            .bind(session)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(expected) = request["conversation_revision"].as_i64() {
        ensure!(
            revision == Some(expected),
            "conflict: discussion candidates changed"
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for turn in turns {
        let turn = turn
            .as_str()
            .filter(|s| !s.is_empty())
            .context("invalid: conversation turn ID")?;
        ensure!(seen.insert(turn), "invalid: duplicate conversation turn");
        let bound: Option<Option<String>> = sqlx::query_scalar(
            "SELECT job_id FROM discussion_turns WHERE session_id=? AND turn_id=?",
        )
        .bind(session)
        .bind(turn)
        .fetch_optional(&mut *conn)
        .await?;
        let bound = bound.context("not_found: discussion turn is absent or expired")?;
        ensure!(
            bound.as_deref().is_none_or(|id| id == job_id),
            "conflict: turn belongs to another Job"
        );
        sqlx::query("UPDATE discussion_turns SET job_id=? WHERE session_id=? AND turn_id=? AND job_id IS NULL")
            .bind(&job_id).bind(session).bind(turn).execute(&mut *conn).await?;
    }
    sqlx::query("UPDATE discussion_sessions SET revision=revision+1 WHERE session_id=?")
        .bind(session)
        .execute(&mut *conn)
        .await?;
    let messages = ordered_messages(conn, session, &job_id).await?;
    let job = &mut state.jobs[index];
    // Adopt the followup placeholder before inserting its preceding discussion.
    if let Some(last) = job
        .conversation
        .last_mut()
        .filter(|m| m.id.starts_with("followup:") && m.session_id == session)
        && let Some(message) = messages
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m["role"] == "user" && m["text"] == last.text)
    {
        last.id = required(message, "id")?.into();
    }
    if request["command"] == "job.create"
        && let Some(first) = messages
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "user")
    {
        job.prompt = required(first, "text")?.into();
    }
    crate::conversation::record(
        state,
        &json!({"session":session,"job":job_id,"messages":messages,"ordered":true}),
        options,
        now,
    )?;
    *result = json!(state.jobs[index]);
    Ok(())
}

impl Store {
    /// Pending turns are read in one snapshot, with an optimistic selection revision.
    pub async fn discussion_list(&self, session: &str, after: i64, limit: i64) -> Result<Value> {
        self.discussion_page(session, after, limit, true).await
    }

    pub async fn discussion_index(&self, session: &str, after: i64, limit: i64) -> Result<Value> {
        self.discussion_page(session, after, limit, false).await
    }

    async fn discussion_page(
        &self,
        session: &str,
        after: i64,
        limit: i64,
        full: bool,
    ) -> Result<Value> {
        ensure!(
            !session.is_empty() && after >= 0 && (1..=100).contains(&limit),
            "invalid: discussion pagination"
        );
        let mut tx = self.pool.begin().await?;
        let revision: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM discussion_sessions WHERE session_id=?")
                .bind(session)
                .fetch_optional(&mut *tx)
                .await?;
        let rows = sqlx::query("SELECT position,turn_id,source,CASE WHEN ? THEN messages ELSE '[]' END AS messages,length(CAST(messages AS BLOB)) AS bytes FROM discussion_turns WHERE session_id=? AND job_id IS NULL AND position>? AND session_id IN (SELECT session_id FROM discussion_sessions WHERE activity>?) ORDER BY position LIMIT ?")
            .bind(full).bind(session).bind(after).bind(self.now()-RETENTION).bind(limit+1).fetch_all(&mut *tx).await?;
        let complete = rows.len() <= usize::try_from(limit)?;
        let turns: Vec<Value> = rows.iter().take(usize::try_from(limit)?).map(|r| {
            let mut turn = json!({"turn_id":r.get::<String,_>("turn_id"),"position":r.get::<i64,_>("position"),"source":r.get::<String,_>("source"),"bytes":r.get::<i64,_>("bytes")});
            if full { turn["messages"] = serde_json::from_str(&r.get::<String,_>("messages"))?; }
            Ok(turn)
        }).collect::<Result<_>>()?;
        tx.commit().await?;
        Ok(
            json!({"revision":revision.unwrap_or(0),"session_id":session,"next_cursor":turns.last().map_or(after,|t|t["position"].as_i64().unwrap()),"complete":complete,"turns":turns}),
        )
    }

    pub async fn discussion_show(&self, session: &str, turn: &str) -> Result<Value> {
        let row = sqlx::query("SELECT turn_id,source,messages,job_id FROM discussion_turns WHERE session_id=? AND turn_id=? AND (job_id IS NOT NULL OR session_id IN (SELECT session_id FROM discussion_sessions WHERE activity>?))")
            .bind(session).bind(turn).bind(self.now()-RETENTION).fetch_one(&self.pool).await?;
        Ok(
            json!({"turn_id":row.get::<String,_>("turn_id"),"source":row.get::<String,_>("source"),"messages":serde_json::from_str::<Value>(&row.get::<String,_>("messages"))?,"job_id":row.get::<Option<String>,_>("job_id")}),
        )
    }
}
