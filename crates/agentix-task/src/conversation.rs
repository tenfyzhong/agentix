use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Snapshot, WriteOptions, mutations::required};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobMessage {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub text: String,
    pub recorded_at: i64,
}

/// Remove complete leading host-context wrappers without filtering requests
/// merely mentioning AGENTS.md or showing wrapper syntax as an example.
pub(crate) fn user_text(mut text: &str) -> &str {
    loop {
        let value = text.trim_start();
        let block = if let Some((_, rest)) = value.split_once('\n').filter(|(heading, _)| {
            *heading == "# AGENTS.md instructions"
                || heading.starts_with("# AGENTS.md instructions for ")
        }) {
            let rest = rest.trim_start();
            if !rest.starts_with("<INSTRUCTIONS>") {
                return text;
            }
            rest
        } else {
            value
        };
        let Some(tag) = [
            "INSTRUCTIONS",
            "environment_context",
            "system-reminder",
            "turn_aborted",
        ]
        .into_iter()
        .find(|tag| block.starts_with(&format!("<{tag}>"))) else {
            return text;
        };
        if tag == "INSTRUCTIONS" && block == value {
            return text;
        }
        let closing = format!("</{tag}>");
        let Some((_, rest)) = block.split_once(&closing) else {
            return text;
        };
        text = rest.trim_start();
    }
}

pub(crate) fn session_job_index(
    state: &Snapshot,
    request: &Value,
    session: &str,
) -> Result<Option<usize>> {
    let associated = |job: &crate::Job| {
        job.session_id.as_deref() == Some(session)
            || job.followup_session_id.as_deref() == Some(session)
            || state
                .tasks
                .iter()
                .any(|task| task.job_id == job.id && task.last_session.as_deref() == Some(session))
    };
    let index = if let Some(id) = request["job"].as_str() {
        let index = state.job_index(id)?;
        ensure!(
            associated(&state.jobs[index]),
            "conflict: Job belongs to another session"
        );
        Some(index)
    } else {
        state
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| associated(job) && job.archived_at.is_none())
            .max_by_key(|(_, job)| {
                let suffix = |id: &str| {
                    id.split_once('_')
                        .map_or(id, |(_, suffix)| suffix)
                        .to_owned()
                };
                let mut activity = (job.created_at, suffix(&job.id));
                for task in state.tasks.iter().filter(|task| {
                    task.job_id == job.id && task.last_session.as_deref() == Some(session)
                }) {
                    activity = activity.max((task.updated_at, suffix(&task.id)));
                }
                if job.followup_session_id.as_deref() == Some(session) {
                    activity = activity.max((
                        job.followup_at.unwrap_or(job.created_at),
                        suffix(job.followup_id.as_deref().unwrap_or(&job.id)),
                    ));
                }
                activity
            })
            .map(|(index, _)| index)
    };
    Ok(index)
}

fn planning_prompt<'a>(request: &'a Value, messages: &[Value]) -> Result<Option<&'a str>> {
    request
        .get("planning")
        .map(|planning| -> Result<&str> {
            let prompt = user_text(required(planning, "prompt")?);
            let implementation = required(planning, "implementation_prompt")?;
            ensure!(
                !prompt.trim().is_empty()
                    && implementation == "Implement the plan."
                    && messages
                        .iter()
                        .find(|m| m["role"] == "user")
                        .and_then(|m| m["text"].as_str())
                        .map(user_text)
                        == Some(prompt)
                    && messages
                        .iter()
                        .any(|m| m["role"] == "user" && m["text"] == implementation),
                "invalid: planning capture must include its original and implementation prompts"
            );
            Ok(prompt)
        })
        .transpose()
}

pub(crate) fn record(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let session = required(request, "session")?;
    ensure!(
        options.session_ref.as_deref() == Some(session),
        "conflict: conversation session mismatch"
    );
    let index = session_job_index(state, request, session)?;
    let Some(index) = index else {
        return Ok(json!({"recorded":0}));
    };
    let messages = request["messages"]
        .as_array()
        .context("invalid: conversation messages")?;
    let job = &mut state.jobs[index];
    let planning_prompt = planning_prompt(request, messages)?;
    let mut prompt_changed = false;
    if let Some(prompt) = planning_prompt
        && (job.prompt.is_empty() || job.prompt == "Implement the plan.")
        && job.prompt != prompt
    {
        job.prompt = prompt.into();
        prompt_changed = true;
    }
    let recorded = merge_messages(
        job,
        messages,
        session,
        now,
        planning_prompt.is_some() || request["ordered"] == true,
        request["before_message"].as_str(),
    )?;
    if recorded > 0 || prompt_changed {
        job.revision += 1;
        job.updated_at = now;
    }
    Ok(json!({"job_id":job.id,"recorded":recorded}))
}

// Compute insertion anchors once, then merge batches without nested scans or
// shifting the existing history once for each new message.
fn merge_messages(
    job: &mut crate::Job,
    messages: &[Value],
    session: &str,
    now: i64,
    ordered: bool,
    before: Option<&str>,
) -> Result<usize> {
    let mut entries = std::mem::take(&mut job.conversation);
    let original_len = entries.len();
    let mut indexes: std::collections::HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.session_id == session)
        .map(|(i, e)| (e.id.clone(), i))
        .collect();
    let mut recorded = 0;
    if let Some(pending) = entries.last_mut()
        && pending.role == "user"
        && pending.session_id == session
        && pending.id.starts_with("followup:")
        && let Some(message) = messages.iter().find(|m| {
            m["role"] == "user"
                && m["text"].as_str().map(user_text) == Some(pending.text.as_str())
                && m["id"].as_str().is_some_and(|id| !indexes.contains_key(id))
        })
    {
        indexes.remove(&pending.id);
        pending.id = required(message, "id")?.to_owned();
        indexes.insert(pending.id.clone(), original_len - 1);
        recorded += 1;
    }
    let mut next = before
        .and_then(|id| indexes.get(id).copied())
        .unwrap_or(original_len);
    let mut anchors = vec![original_len; messages.len()];
    if ordered {
        for (i, message) in messages.iter().enumerate().rev() {
            anchors[i] = next;
            if let Some(index) = message["id"].as_str().and_then(|id| indexes.get(id)) {
                next = *index;
            }
        }
    }
    let mut inserted = Vec::new();
    for (position, message) in messages.iter().enumerate() {
        let id = required(message, "id")?;
        let role = required(message, "role")?;
        ensure!(
            matches!(role, "user" | "assistant"),
            "invalid: only user and assistant text may be recorded"
        );
        let text = required(message, "text")?;
        let text = if role == "user" {
            user_text(text)
        } else {
            text
        };
        if text.trim().is_empty() {
            continue;
        }
        if let Some(&index) = indexes.get(id) {
            let existing = &mut entries[index];
            ensure!(existing.role == role, "conflict: message role changed");
            if existing.text == text {
                continue;
            }
            existing.text = text.into();
        } else {
            indexes.insert(id.into(), entries.len());
            entries.push(JobMessage {
                id: id.into(),
                session_id: session.into(),
                role: role.into(),
                text: text.into(),
                recorded_at: now,
            });
            inserted.push(anchors[position]);
        }
        if job.prompt.is_empty() && role == "user" {
            job.prompt = text.into();
        }
        recorded += 1;
    }
    if inserted.is_empty() {
        job.conversation = entries;
    } else {
        let mut buckets = vec![Vec::new(); original_len + 1];
        for (entry, anchor) in entries.drain(original_len..).zip(inserted) {
            buckets[anchor].push(entry);
        }
        let mut merged = Vec::with_capacity(original_len + messages.len());
        for (entry, bucket) in entries.into_iter().zip(&mut buckets) {
            merged.append(bucket);
            merged.push(entry);
        }
        merged.append(&mut buckets[original_len]);
        job.conversation = merged;
    }
    Ok(recorded)
}
