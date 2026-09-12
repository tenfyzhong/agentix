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
        let block = if let Some(rest) = value.strip_prefix("# AGENTS.md instructions\n") {
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

fn session_job_index(state: &Snapshot, request: &Value, session: &str) -> Result<Option<usize>> {
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
    let mut recorded = 0;
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
        // Deduplicate historical IDs before adopting a followup placeholder.
        // Identical wording from an earlier turn must not claim the new prompt.
        if let Some(existing) = job
            .conversation
            .iter_mut()
            .find(|entry| entry.id == id && entry.session_id == session)
        {
            ensure!(existing.role == role, "conflict: message role changed");
            if existing.text == text {
                continue;
            }
            existing.text = text.into();
        } else if role == "user"
            && let Some(pending) = job.conversation.last_mut()
            && pending.role == "user"
            && pending.id.starts_with("followup:")
            && pending.session_id == session
            && pending.text == text
        {
            pending.id = id.into();
        } else {
            // The live bridge may already have recorded the execution turn.
            // Insert recovered planning messages before the next known message
            // in this ordered batch, preserving all other conversation entries.
            let next = planning_prompt
                .and_then(|_| {
                    messages[position + 1..].iter().find_map(|following| {
                        job.conversation.iter().position(|entry| {
                            entry.session_id == session && following["id"] == entry.id
                        })
                    })
                })
                .unwrap_or(job.conversation.len());
            job.conversation.insert(
                next,
                JobMessage {
                    id: id.into(),
                    session_id: session.into(),
                    role: role.into(),
                    text: text.into(),
                    recorded_at: now,
                },
            );
        }
        if job.prompt.is_empty() && role == "user" {
            job.prompt = text.into();
        }
        recorded += 1;
    }
    if recorded > 0 || prompt_changed {
        job.revision += 1;
        job.updated_at = now;
    }
    Ok(json!({"job_id":job.id,"recorded":recorded}))
}
