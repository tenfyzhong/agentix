//! Channel-neutral view construction. No storage, adapters or async I/O.
use super::{HistoryPresentation, InputOption, InputProgress, InputQuestion, TurnBuffer};
use crate::{
    ChannelCommand, CommandMenu, DeliveryClass, HistoryPage, InteractionRequest, MultiplexerPane,
    MultiplexerSession, MultiplexerSnapshot, MultiplexerWindow, OutboundView, SessionId,
    SessionStatus, SessionSummary, TurnStatus, TurnSummary, ViewStatus,
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub(super) fn multiplexer_session_contains(
    session: &MultiplexerSession,
    current: Option<&SessionId>,
) -> bool {
    session
        .windows
        .iter()
        .any(|window| multiplexer_window_contains(window, current))
}

pub(super) fn multiplexer_window_contains(
    window: &MultiplexerWindow,
    current: Option<&SessionId>,
) -> bool {
    window
        .panes
        .iter()
        .any(|pane| current.is_some_and(|current| pane.agent_session.as_ref() == Some(current)))
}

pub(super) fn multiplexer_root_body(
    snapshot: &MultiplexerSnapshot,
    current: Option<&SessionId>,
) -> (String, usize, usize) {
    let window_count = snapshot
        .sessions
        .iter()
        .map(|session| session.windows.len())
        .sum();
    let pane_count = snapshot
        .sessions
        .iter()
        .flat_map(|session| &session.windows)
        .map(|window| window.panes.len())
        .sum();
    let body = snapshot
        .sessions
        .iter()
        .map(|session| {
            let panes = session
                .windows
                .iter()
                .map(|window| window.panes.len())
                .sum::<usize>();
            markdown_quote(&format!(
                "**{}**{}\n{} {} · {panes} {}",
                session.name,
                if multiplexer_session_contains(session, current) {
                    " · 📎 **Attached**"
                } else {
                    ""
                },
                session.windows.len(),
                plural(session.windows.len(), "window", "windows"),
                plural(panes, "pane", "panes")
            ))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    (body, window_count, pane_count)
}

pub(super) fn multiplexer_session_body(
    session: &MultiplexerSession,
    current: Option<&SessionId>,
) -> String {
    session
        .windows
        .iter()
        .map(|window| {
            markdown_quote(&format!(
                "**{} · {}**{}\n{} {}",
                window.index,
                window.name,
                if multiplexer_window_contains(window, current) {
                    " · 📎 **Attached**"
                } else {
                    ""
                },
                window.panes.len(),
                plural(window.panes.len(), "pane", "panes")
            ))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn multiplexer_window_body(
    window: &MultiplexerWindow,
    current: Option<&SessionId>,
    backend_name: &str,
) -> String {
    window
        .panes
        .iter()
        .map(|pane| multiplexer_pane_body(pane, current, backend_name))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn multiplexer_pane_body(
    pane: &MultiplexerPane,
    current: Option<&SessionId>,
    backend_name: &str,
) -> String {
    let attached = current.is_some_and(|current| pane.agent_session.as_ref() == Some(current));
    let state = if attached {
        "📎 Attached"
    } else if let Some(session) = &pane.agent_session {
        crate::SessionRef::decode(session)
            .map_or(backend_name, |session| session.agent.display_name())
    } else if is_shell_command(&pane.current_command) {
        "Idle shell"
    } else {
        "Busy"
    };
    markdown_quote(&format!(
        "**{} · {}**{} · {state}\n📁 `{}`",
        pane.index,
        pane.current_command,
        if pane.active { " · Active" } else { "" },
        display_workspace(Some(&pane.cwd))
    ))
}

pub(super) fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

pub(super) fn is_shell_command(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "bash" | "dash" | "elvish" | "fish" | "ksh" | "nu" | "sh" | "tcsh" | "zsh"
            )
        })
}

/// Unified built-in commands, also used by adapters with app-wide menus.
#[must_use]
pub fn command_menu(attached: bool) -> CommandMenu {
    command_menu_for(attached, crate::MultiplexerKind::default())
}

#[must_use]
pub fn command_menu_for(
    attached: bool,
    kind: impl Into<Option<crate::MultiplexerKind>>,
) -> CommandMenu {
    let kind = kind.into();
    let mut commands = [
        ("sessions", "Browse running sessions"),
        ("cancel", "Cancel pending input"),
        ("help", "Show available commands"),
    ]
    .into_iter()
    .map(|(name, description)| ChannelCommand::new(name, description))
    .collect::<Vec<_>>();
    if let Some(kind) = kind {
        commands.insert(
            1,
            ChannelCommand::new(kind.as_str(), "Manage terminal workspaces"),
        );
    }
    if attached {
        commands.splice(
            2..2,
            [
                ("current", "Show the attached session"),
                ("history", "Show recent conversation history"),
                ("queue", "Show queued follow-up messages"),
                ("stop", "Stop the active turn"),
                ("detach", "Detach the current session"),
                ("compact", "Compact the session context"),
                ("fork", "Fork and attach a copy"),
                ("fast", "Toggle Fast mode"),
                ("clear", "Start a fresh session"),
                ("exit", "Detach the session"),
                ("diff", "Show Git changes"),
                ("rename", "Rename the session"),
                ("model", "Show or change the model"),
                ("reasoning", "Show or change reasoning effort"),
                ("skills", "List available skills"),
                ("plan", "Enter plan mode"),
                ("goal", "Show or manage the goal"),
                ("review", "Review uncommitted changes"),
                ("status", "Show detailed session status"),
                ("mcp", "Show MCP server status"),
            ]
            .into_iter()
            .map(|(name, description)| ChannelCommand::new(name, description).contextual()),
        );
    }
    CommandMenu::new(commands)
}

pub(super) fn history_views(
    agent_name: &str,
    session_label: &str,
    history: &HistoryPage,
    presentation: HistoryPresentation,
) -> Vec<OutboundView> {
    let turn_count = history.turns.len();
    let mut body = if turn_count == 0 {
        "No conversation history yet.".to_owned()
    } else {
        format!(
            "Showing {turn_count} recent {}.",
            if turn_count == 1 { "turn" } else { "turns" }
        )
    };
    if history.older_cursor.is_some() {
        body.push_str("\n\nEarlier turns: /history older");
    }
    if history.newer_cursor.is_some() {
        body.push_str("\nNewer turns: /history newer");
    }

    let subtitle = match presentation {
        HistoryPresentation::Attached => "Attached",
        HistoryPresentation::History => "History",
    };
    let mut views = vec![OutboundView {
        sections: Vec::new(),
        title: format!("{agent_name} · {session_label}"),
        subtitle: Some(subtitle.into()),
        body,
        status: ViewStatus::Info,
        actions: Vec::new(),
    }];
    views.extend(
        history
            .turns
            .iter()
            .map(|turn| history_turn_view(agent_name, turn)),
    );
    views
}

pub(super) fn history_turn_view(agent_name: &str, turn: &TurnSummary) -> OutboundView {
    let body = turn_conversation_body(
        agent_name,
        turn.user_text.as_deref(),
        turn.agent_text.as_deref(),
    );

    OutboundView {
        sections: Vec::new(),
        title: format!("{agent_name} · Turn {}", short_identifier(&turn.id)),
        subtitle: Some(turn_status_label(&turn.status).into()),
        body,
        status: match turn.status {
            TurnStatus::Completed => ViewStatus::Success,
            TurnStatus::Failed => ViewStatus::Error,
            TurnStatus::Interrupted => ViewStatus::Warning,
            TurnStatus::InProgress | TurnStatus::Unknown => ViewStatus::Running,
        },
        actions: Vec::new(),
    }
}

pub(super) fn live_turn_body(
    agent_name: &str,
    buffer: &TurnBuffer,
    delivery: DeliveryClass,
) -> String {
    let output = buffer.render_output();
    let mut body = turn_conversation_body(agent_name, Some(&buffer.user_text), Some(&output));
    if delivery == DeliveryClass::Draining {
        body.push_str("\n\nThis is a background session after switching.");
    }
    body
}

pub(super) fn live_turn_view(
    agent_name: &str,
    session_label: &str,
    turn_id: &str,
    buffer: &TurnBuffer,
    delivery: DeliveryClass,
) -> OutboundView {
    OutboundView {
        sections: buffer.view_sections(agent_name),
        title: format!("{agent_name} · {session_label}"),
        subtitle: Some(format!(
            "{} {} · {}",
            if delivery == DeliveryClass::Draining {
                "Background turn"
            } else {
                "Turn"
            },
            short_identifier(turn_id),
            live_turn_status_label(buffer)
        )),
        body: live_turn_body(agent_name, buffer, delivery),
        status: if delivery == DeliveryClass::Draining {
            ViewStatus::Background
        } else {
            turn_view_status(&buffer.status)
        },
        actions: Vec::new(),
    }
}

pub(super) fn background_completion_body(status: &TurnStatus, error: Option<&str>) -> String {
    let outcome = match status {
        TurnStatus::Completed => "completed",
        TurnStatus::Failed => "failed",
        TurnStatus::Interrupted => "was interrupted",
        TurnStatus::InProgress | TurnStatus::Unknown => "finished",
    };
    let mut body =
        format!("This turn {outcome} in a session that is not attached to this IM conversation.");
    if let Some(error) = error.filter(|error| !error.trim().is_empty()) {
        body.push_str(&format!("\n\n**Error**\n\n{}", markdown_quote(error)));
    }
    body
}

pub(super) const fn turn_view_status(status: &TurnStatus) -> ViewStatus {
    match status {
        TurnStatus::Completed => ViewStatus::Success,
        TurnStatus::Failed => ViewStatus::Error,
        TurnStatus::Interrupted => ViewStatus::Warning,
        TurnStatus::InProgress | TurnStatus::Unknown => ViewStatus::Running,
    }
}

pub(super) fn turn_conversation_body(
    agent_name: &str,
    user_text: Option<&str>,
    agent_text: Option<&str>,
) -> String {
    let mut sections = Vec::new();
    if let Some(user_text) = user_text.filter(|text| !text.trim().is_empty()) {
        sections.push(format!("**👤 You**\n\n{}", markdown_quote(user_text)));
    }
    if let Some(agent_text) = agent_text.filter(|text| !text.trim().is_empty()) {
        sections.push(format!(
            "**🤖 {agent_name}**\n\n{}",
            markdown_quote(agent_text)
        ));
    }
    if sections.is_empty() {
        "No text content.".to_owned()
    } else {
        sections.join("\n\n")
    }
}

pub(super) fn markdown_quote(text: &str) -> String {
    text.trim()
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn short_identifier(value: &str) -> &str {
    &value[..value.floor_char_boundary(8)]
}

pub(super) fn turn_status_label(status: &TurnStatus) -> &'static str {
    match status {
        TurnStatus::InProgress => "In progress",
        TurnStatus::Completed => "Completed",
        TurnStatus::Interrupted => "Interrupted",
        TurnStatus::Failed => "Failed",
        TurnStatus::Unknown => "Unknown",
    }
}

pub(super) fn live_turn_status_label(buffer: &TurnBuffer) -> String {
    let elapsed = buffer.elapsed().map(format_elapsed);
    match (&buffer.status, elapsed) {
        (TurnStatus::InProgress | TurnStatus::Unknown, Some(elapsed)) => {
            format!("Working {elapsed}")
        }
        (TurnStatus::Completed, Some(elapsed)) => format!("Completed in {elapsed}"),
        (TurnStatus::Interrupted, Some(elapsed)) => format!("Interrupted after {elapsed}"),
        (TurnStatus::Failed, Some(elapsed)) => format!("Failed after {elapsed}"),
        (status, None) => turn_status_label(status).to_owned(),
    }
}

pub(super) fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

pub(super) fn display_workspace(workspace: Option<&str>) -> String {
    let Some(workspace) = workspace else {
        return "unknown workspace".to_owned();
    };
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return workspace.to_owned();
    };
    let Ok(relative) = Path::new(workspace).strip_prefix(Path::new(&home)) else {
        return workspace.to_owned();
    };
    if relative.as_os_str().is_empty() {
        "~".to_owned()
    } else {
        format!("~/{}", relative.display())
    }
}

pub(super) fn session_status_label(status: &SessionStatus) -> (&'static str, &'static str) {
    match status {
        SessionStatus::Active => ("🟢", "Active"),
        SessionStatus::Idle => ("🟡", "Idle"),
        SessionStatus::NotLoaded => ("⚫", "Not loaded"),
        SessionStatus::SystemError => ("⚠️", "System error"),
        SessionStatus::Offline => ("🔴", "Offline"),
        SessionStatus::Unknown => ("⚪", "Unknown"),
    }
}

pub(super) fn session_title(session: &SessionSummary) -> &str {
    session
        .name
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .or_else(|| {
            session
                .preview
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
        })
        .unwrap_or("Untitled")
}

pub(super) fn session_display_label(session: &SessionSummary) -> String {
    format!("{} · {}", session_title(session), session.id.short())
}

pub(super) fn input_questions(request: &InteractionRequest) -> Vec<InputQuestion> {
    let mut questions = request
        .payload
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, question)| {
            let id = question.get("id").and_then(Value::as_str)?.to_owned();
            let header = question
                .get("header")
                .and_then(Value::as_str)
                .filter(|header| !header.trim().is_empty())
                .map_or_else(|| format!("Question {}", index + 1), str::to_owned);
            let prompt = question
                .get("question")
                .and_then(Value::as_str)
                .filter(|prompt| !prompt.trim().is_empty())
                .unwrap_or(&request.detail)
                .to_owned();
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    Some(InputOption {
                        label: option.get("label")?.as_str()?.to_owned(),
                        description: option
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })
                })
                .collect();
            Some(InputQuestion {
                id,
                header,
                question: prompt,
                options,
                secret: question
                    .get("isSecret")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    if questions.is_empty() {
        questions.push(InputQuestion {
            id: "value".into(),
            header: "Input".into(),
            question: if request.detail.trim().is_empty() {
                "Please provide an answer.".into()
            } else {
                request.detail.clone()
            },
            options: Vec::new(),
            secret: false,
        });
    }
    questions
}

pub(super) fn input_progress_body(progress: &InputProgress, awaiting_custom: bool) -> String {
    let mut sections = Vec::new();
    let answered = progress
        .questions
        .iter()
        .zip(&progress.answers)
        .enumerate()
        .filter_map(|(index, (question, answer))| {
            answer.as_ref().map(|answer| {
                format!(
                    "{}. **{}:** {}",
                    index + 1,
                    question.header,
                    displayed_answer(question, answer)
                )
            })
        })
        .collect::<Vec<_>>();
    if !answered.is_empty() {
        sections.push(format!("**Answers**\n{}", answered.join("\n")));
    }
    if let Some(question) = progress.questions.get(progress.current) {
        let mut current = format!(
            "**Question {} of {} · {}**\n{}",
            progress.current + 1,
            progress.questions.len(),
            question.header,
            question.question
        );
        if !question.options.is_empty() {
            let options = question
                .options
                .iter()
                .map(|option| {
                    if option.description.trim().is_empty() {
                        format!("- **{}**", option.label)
                    } else {
                        format!("- **{}** — {}", option.label, option.description)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            current.push_str(&format!("\n\n{options}"));
        }
        current.push_str(if awaiting_custom {
            "\n\nReply with your custom answer. Use `/cancel` to return to the choices."
        } else if question.options.is_empty() {
            "\n\nChoose Answer… to type your response."
        } else {
            "\n\nChoose an option below, or choose Other… to type a custom answer."
        });
        sections.push(current);
    }
    sections.join("\n\n")
}

pub(super) fn completed_input_body(progress: &InputProgress) -> String {
    let answers = progress
        .questions
        .iter()
        .zip(&progress.answers)
        .enumerate()
        .filter_map(|(index, (question, answer))| {
            answer.as_ref().map(|answer| {
                format!(
                    "{}. **{}:** {}",
                    index + 1,
                    question.header,
                    displayed_answer(question, answer)
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("**Answered**\n{answers}")
}

pub(super) fn displayed_answer<'a>(question: &InputQuestion, answer: &'a str) -> &'a str {
    if question.secret { "[hidden]" } else { answer }
}

pub(super) fn input_response(progress: &InputProgress) -> Value {
    let answers = progress
        .questions
        .iter()
        .zip(&progress.answers)
        .filter_map(|(question, answer)| {
            answer
                .as_ref()
                .map(|answer| (question.id.clone(), json!({"answers": [answer]})))
        })
        .collect::<serde_json::Map<_, _>>();
    json!({"answers": answers})
}

pub(super) fn decision_label(decision: &str) -> String {
    match decision {
        "accept" => "Allow once",
        "acceptForSession" => "Allow for session",
        "decline" => "Decline",
        "cancel" => "Cancel",
        other => other,
    }
    .to_owned()
}
