//! Conservative recognition of common terminal UI shapes, not command names.
use crate::native_control::{run, styled_lines};
use crate::{MultiplexerDriver, MultiplexerError};
use agentix_domain::{
    AgentError, AgentKind, TerminalInteraction, TerminalInteractionKind as Kind,
    TerminalInteractionPort, TerminalInteractionResponse as Response, TerminalInteractionTarget,
};
use async_trait::async_trait;
use std::{path::Path, sync::Arc, time::Duration};

fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn menu_row(text: &str) -> Option<(usize, bool, String)> {
    let text = text.trim();
    let (text, selected) = ['›', '❯', '>']
        .iter()
        .find_map(|marker| text.strip_prefix(*marker))
        .map_or((text, false), |text| (text.trim_start(), true));
    let (number, label) = text.split_once(". ")?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) || label.trim().is_empty() {
        return None;
    }
    Some((number.parse().ok()?, selected, compact(label)))
}

fn border(row: &str) -> bool {
    let row = row.trim();
    !row.is_empty()
        && row
            .chars()
            .all(|c| ['─', '━', '═', '┌', '┐', '└', '┘', '│'].contains(&c))
}

fn dialog_hint(row: &str) -> bool {
    let lower = row.to_ascii_lowercase();
    lower.contains("esc")
        && (lower.contains("cancel") || lower.contains("close") || lower.contains("back"))
        && (lower.contains("enter") || lower.contains("select") || lower.contains("submit"))
}

fn cursor_in_menu(rows: &[String], last: usize, cursor_row: usize) -> bool {
    cursor_row <= last
        || rows[last + 1..].iter().enumerate().any(|(i, row)| {
            dialog_hint(row)
                && cursor_row <= last + 1 + i
                && rows[last + 2 + i..]
                    .iter()
                    .all(|row| row.trim().is_empty() || border(row))
        })
}

fn arrow_menu(rows: &[String], cursor_row: usize) -> Option<TerminalInteraction> {
    let selected = rows
        .iter()
        .position(|row| row.trim_start().starts_with("→ "))?;
    let first = (0..=selected)
        .rev()
        .take_while(|i| !rows[*i].trim().is_empty() && !border(&rows[*i]))
        .last()?;
    let last = (selected..rows.len())
        .take_while(|i| !rows[*i].trim().is_empty() && !border(&rows[*i]))
        .last()?;
    let hint = rows[last + 1..].iter().position(|row| dialog_hint(row))? + last + 1;
    if cursor_row > hint
        || rows[hint + 1..]
            .iter()
            .any(|row| !row.trim().is_empty() && !border(row))
        || !(2..=20).contains(&(last - first + 1))
    {
        return None;
    }
    let title = compact(
        rows[..first]
            .iter()
            .rev()
            .find(|row| !row.trim().is_empty() && !border(row))?,
    );
    let options = rows[first..=last]
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let text = row.trim();
            if text.contains("[ ]") || text.contains("[x]") {
                return None;
            }
            Some(format!(
                "{}. {}",
                i + 1,
                compact(text.strip_prefix("→ ").unwrap_or(text))
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    if rows[first..=last]
        .iter()
        .filter(|row| row.trim_start().starts_with("→ "))
        .count()
        != 1
    {
        return None;
    }
    let detail = options.join("\n");
    Some(TerminalInteraction {
        kind: Kind::Choice,
        title: title.clone(),
        detail: detail.clone(),
        choices: options,
        selected: Some(selected - first),
        fingerprint: format!("arrow\n{title}\n{detail}"),
        pane_id: String::new(),
    })
}

/// Parse only a live-looking dialog. Unrecognized dialog shapes are read-only.
#[must_use]
pub fn parse_terminal_interaction(screen: &str, cursor_row: usize) -> Option<TerminalInteraction> {
    let rows = styled_lines(screen)
        .ok()?
        .into_iter()
        .map(|row| row.0)
        .collect::<Vec<_>>();
    if let Some(prompt) = arrow_menu(&rows, cursor_row) {
        return Some(prompt);
    }
    let selected_row = rows
        .iter()
        .position(|row| menu_row(row).is_some_and(|(_, selected, _)| selected));
    if let Some(selected_row) = selected_row {
        let first = (0..=selected_row)
            .rev()
            .take_while(|i| menu_row(&rows[*i]).is_some())
            .last()?;
        let last = (first..rows.len())
            .take_while(|i| menu_row(&rows[*i]).is_some())
            .last()?;
        let choices = rows[first..=last]
            .iter()
            .map(|row| menu_row(row).unwrap())
            .collect::<Vec<_>>();
        let composer_after = rows[last + 1..].iter().any(|row| {
            let text = row.trim_start();
            ['›', '❯', '>']
                .iter()
                .any(|marker| text.starts_with(*marker))
                && menu_row(text).is_none()
        });
        if choices.len() >= 2
            && choices.len() <= 20
            && cursor_in_menu(&rows, last, cursor_row)
            && !composer_after
            && choices.iter().enumerate().all(|(i, row)| row.0 == i + 1)
            && choices.iter().filter(|row| row.1).count() == 1
        {
            let title_row = rows[..first]
                .iter()
                .rposition(|row| !row.trim().is_empty())?;
            let title = compact(&rows[title_row]);
            let labels = choices
                .iter()
                .map(|row| format!("{}. {}", row.0, row.2))
                .collect::<Vec<_>>();
            let detail = labels.join("\n");
            return Some(TerminalInteraction {
                kind: Kind::Choice,
                title: title.clone(),
                detail: detail.clone(),
                choices: labels,
                selected: Some(selected_row - first),
                fingerprint: format!("choice\n{title}\n{detail}"),
                pane_id: String::new(),
            });
        }
    }
    if let Some(row) = rows.get(cursor_row) {
        let text = compact(row);
        let lower = text.to_ascii_lowercase();
        if ["[y/n]", "(y/n)"]
            .iter()
            .any(|ending| lower.ends_with(ending))
            && rows[cursor_row + 1..]
                .iter()
                .all(|row| row.trim().is_empty())
        {
            return Some(TerminalInteraction {
                kind: Kind::Confirmation,
                title: text.clone(),
                detail: text.clone(),
                choices: vec!["Yes".into(), "No".into()],
                selected: None,
                fingerprint: format!("confirmation\n{text}"),
                pane_id: String::new(),
            });
        }
    }
    let hint = rows.iter().rposition(|row| dialog_hint(row))?;
    if cursor_row > hint
        || rows[hint + 1..]
            .iter()
            .any(|row| !row.trim().is_empty() && !border(row))
    {
        return None;
    }
    let detail = rows.join("\n").trim().to_owned();
    if detail.len() > 32 * 1024 {
        return None;
    }
    Some(TerminalInteraction {
        kind: Kind::Unknown,
        title: "Terminal needs attention".into(),
        detail: detail.clone(),
        choices: Vec::new(),
        selected: None,
        fingerprint: format!("unknown\n{detail}"),
        pane_id: String::new(),
    })
}

fn failure(message: impl std::fmt::Display) -> MultiplexerError {
    MultiplexerError::Backend(message.to_string())
}

async fn screen(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
    agent: AgentKind,
) -> Result<(String, usize), MultiplexerError> {
    let process = run(
        Path::new("ps"),
        &[],
        &["-p", &pid.to_string(), "-o", "pgid=,tpgid=,comm="],
    )
    .await?;
    let parts = process.split_whitespace().collect::<Vec<_>>();
    let names: &[&str] = match agent {
        AgentKind::Codex => &["codex"],
        AgentKind::Pi => &["pi", "node", "bun"],
        AgentKind::Omp => &["omp", "node", "bun"],
        AgentKind::Claude => &["claude", "node"],
    };
    if parts.len() < 3
        || parts[0] != parts[1]
        || ["0", "-1"].contains(&parts[0])
        || !Path::new(parts[2])
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| names.contains(&name))
    {
        return Err(failure(
            "Original agent process is no longer in the foreground",
        ));
    }
    let state = run(
        command,
        prefix,
        &[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{pane_current_command}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}",
        ],
    )
    .await?;
    let parts = state.trim().split('|').collect::<Vec<_>>();
    if parts.len() != 4 || !names.contains(&parts[0]) || parts[2] != "0" || parts[3] != "0" {
        return Err(failure(
            "Original agent pane is unavailable or in copy mode",
        ));
    }
    let cursor = parts[1].parse().map_err(failure)?;
    let screen = run(command, prefix, &["capture-pane", "-p", "-e", "-t", pane]).await?;
    Ok((screen, cursor))
}

pub async fn inspect_terminal_interaction(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
    agent: AgentKind,
) -> Result<Option<TerminalInteraction>, MultiplexerError> {
    let (text, cursor) = screen(command, prefix, pane, pid, agent).await?;
    Ok(parse_terminal_interaction(&text, cursor).map(|mut prompt| {
        prompt.pane_id = pane.into();
        prompt
    }))
}

pub async fn respond_terminal_interaction(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
    agent: AgentKind,
    expected: &TerminalInteraction,
    response: Response,
) -> Result<(), MultiplexerError> {
    if expected.kind == Kind::Unknown {
        return Err(failure(
            "Answer this custom dialog in the original terminal",
        ));
    }
    if let Response::Choice(index) = response
        && index >= expected.choices.len()
    {
        return Err(failure("Invalid terminal choice"));
    }
    let mut ready = false;
    for _ in 0..40 {
        let current = inspect_terminal_interaction(command, prefix, pane, pid, agent)
            .await?
            .ok_or_else(|| failure("Terminal interaction expired; no input was sent"))?;
        if current.fingerprint != expected.fingerprint || current.pane_id != expected.pane_id {
            return Err(failure(
                "Terminal interaction changed; no further input was sent",
            ));
        }
        let key = match (current.kind, response) {
            (Kind::Confirmation, Response::Choice(0)) => "y",
            (Kind::Confirmation, _) => "n",
            (Kind::Choice, Response::Cancel) => "Escape",
            (Kind::Choice, Response::Choice(index)) if current.selected == Some(index) && ready => {
                "Enter"
            }
            (Kind::Choice, Response::Choice(index)) if current.selected == Some(index) => {
                ready = true;
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            (Kind::Choice, Response::Choice(index)) => {
                ready = false;
                if current.selected.is_some_and(|selected| selected > index) {
                    "Up"
                } else {
                    "Down"
                }
            }
            _ => return Err(failure("Unsupported terminal interaction")),
        };
        run(command, prefix, &["send-keys", "-t", pane, key]).await?;
        if !["Up", "Down"].contains(&key) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(failure(
        "Terminal did not select the requested option; Enter was not sent",
    ))
}

pub struct TerminalInteractions {
    driver: Arc<dyn MultiplexerDriver>,
}
impl TerminalInteractions {
    #[must_use]
    pub fn new(driver: Arc<dyn MultiplexerDriver>) -> Self {
        Self { driver }
    }
}
#[async_trait]
impl TerminalInteractionPort for TerminalInteractions {
    async fn inspect(
        &self,
        agent: AgentKind,
        target: &TerminalInteractionTarget,
    ) -> Result<Option<TerminalInteraction>, AgentError> {
        self.driver
            .inspect_interaction(agent, target.pid)
            .await
            .map_err(|e| AgentError::Unavailable(e.to_string()))
    }
    async fn respond(
        &self,
        agent: AgentKind,
        target: &TerminalInteractionTarget,
        expected: &TerminalInteraction,
        response: Response,
    ) -> Result<(), AgentError> {
        self.driver
            .respond_interaction(agent, target.pid, expected, response)
            .await
            .map_err(|e| AgentError::Rejected(e.to_string()))
    }
}
