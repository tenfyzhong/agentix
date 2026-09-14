//! Recover native goal prompts omitted by the app-server turn projection.
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

use agentix_domain::{HistoryPage, SessionId};
use serde_json::Value;

use super::CodexClient;

impl CodexClient {
    pub(super) async fn restore_goal_inputs(
        &self,
        session: &SessionId,
        page: &mut HistoryPage,
        rollout_path: Option<&str>,
    ) {
        let missing = page
            .turns
            .iter()
            .filter(|turn| {
                turn.user_text
                    .as_deref()
                    .is_none_or(|text| text.trim().is_empty())
            })
            .map(|turn| turn.id.clone())
            .collect::<HashSet<_>>();
        if missing.is_empty() {
            return;
        }
        let prompts = self.goal_inputs(session, missing, rollout_path).await;
        for turn in &mut page.turns {
            if let Some(prompt) = prompts.get(&turn.id) {
                turn.user_text = Some(prompt.clone());
            }
        }
    }

    pub(super) async fn goal_inputs(
        &self,
        session: &SessionId,
        wanted: HashSet<String>,
        rollout_path: Option<&str>,
    ) -> HashMap<String, String> {
        let path = if let Some(path) = rollout_path {
            path.to_owned()
        } else {
            let Ok(thread) = self.read_thread(session, false).await else {
                return HashMap::new();
            };
            let Some(path) = thread["path"].as_str() else {
                return HashMap::new();
            };
            path.to_owned()
        };
        let session = session.to_string();
        // Use only this thread's advertised path and exact turn IDs. A missing
        // local rollout (including remote servers) must not break IM output.
        tokio::task::spawn_blocking(move || read_goal_inputs(Path::new(&path), &session, &wanted))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    }
}

fn read_goal_inputs(
    path: &Path,
    session: &str,
    wanted: &HashSet<String>,
) -> std::io::Result<HashMap<String, String>> {
    let reader = BufReader::new(std::fs::File::open(path)?);
    let mut prompts = HashMap::new();
    let mut verified = false;
    let mut turn_id = None;
    for line in reader.lines() {
        let line = line?;
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = &entry["payload"];
        match entry["type"].as_str() {
            Some("session_meta") => {
                if payload["id"].as_str() != Some(session) {
                    return Ok(HashMap::new());
                }
                verified = true;
            }
            Some("event_msg") if verified => match payload["type"].as_str() {
                Some("task_started") => {
                    turn_id = payload["turn_id"].as_str().map(str::to_owned);
                }
                Some("task_complete" | "turn_aborted") => turn_id = None,
                _ => {}
            },
            Some("response_item")
                if verified && payload["type"] == "message" && payload["role"] == "user" =>
            {
                let Some(id) = turn_id.as_ref().filter(|id| wanted.contains(*id)) else {
                    continue;
                };
                for part in payload["content"].as_array().into_iter().flatten() {
                    if let Some(objective) = part["text"].as_str().and_then(goal_objective) {
                        prompts
                            .entry(id.clone())
                            .or_insert_with(|| format!("/goal {objective}"));
                    }
                }
                if prompts.len() == wanted.len() {
                    break;
                }
            }
            _ => {}
        }
    }
    Ok(prompts)
}

fn goal_objective(text: &str) -> Option<&str> {
    let context = text
        .trim()
        .strip_prefix("<codex_internal_context source=\"goal\">")?
        .strip_suffix("</codex_internal_context>")?;
    let (_, objective) = context.split_once("\n<objective>\n")?;
    let (objective, _) = objective.rsplit_once("\n</objective>")?;
    let objective = objective.trim();
    (!objective.is_empty()).then_some(objective)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn goal_context_extracts_only_objective_and_preserves_multiline_text() {
        assert_eq!(
            goal_objective(
                "<codex_internal_context source=\"goal\">\nInternal\n<objective>\nReview\n修复问题\n</objective>\nInternal\n</codex_internal_context>"
            ),
            Some("Review\n修复问题")
        );
        for text in [
            "<objective>Do this</objective>",
            "<codex_internal_context source=\"other\">\n<objective>\nNo\n</objective>\n</codex_internal_context>",
            "<codex_internal_context source=\"goal\">\n<objective>\n \n</objective>\n</codex_internal_context>",
            "<codex_internal_context source=\"goal\">\n<objective>\nIncomplete",
        ] {
            assert_eq!(goal_objective(text), None);
        }
    }

    #[test]
    fn rollout_goal_input_requires_matching_session_and_active_turn() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let message = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"<codex_internal_context source=\"goal\">\n<objective>\nObjective\n</objective>\n</codex_internal_context>"}]}});
        let entries = [
            json!({"type":"session_meta","payload":{"id":"session"}}),
            message.clone(),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"goal"}}),
            message.clone(),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"normal"}}),
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"normal"}}),
            message,
        ];
        std::fs::write(
            file.path(),
            entries
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let wanted = HashSet::from(["goal".into(), "normal".into()]);
        assert_eq!(
            read_goal_inputs(file.path(), "session", &wanted).unwrap(),
            HashMap::from([("goal".into(), "/goal Objective".into())])
        );
        assert!(
            read_goal_inputs(file.path(), "other-session", &wanted)
                .unwrap()
                .is_empty()
        );
    }
}
