//! Conversation-scoped, one-shot directory selection for terminal creation.
use super::{
    ActionButton, ActionStyle, ConversationRef, Engine, EngineError, MultiplexerMutation,
    MultiplexerTarget, MultiplexerUiAction, OutboundView, ParsedInput, UiAction, Uuid,
};

#[derive(Debug, Clone)]
pub(super) enum DirectoryAction {
    Preview,
    Browse {
        path: String,
        page: usize,
        hidden: bool,
    },
    Select(String),
    Input,
    Confirm,
    Cancel,
}

#[derive(Debug, Clone)]
pub(super) struct DirectoryDraft {
    id: String,
    owner: String,
    generation: u64,
    workspace_generation: u64,
    epoch: u64,
    backend: Option<crate::AgentKind>,
    mutation: MultiplexerMutation,
    directory: String,
    home: String,
    browse_directory: String,
    source: String,
    candidates: Vec<String>,
    awaiting_input: bool,
    expired: bool,
}

impl Engine {
    pub(super) async fn expire_directory_drafts(&self) {
        // Retain a tombstone so the next pending path cannot become an agent prompt.
        for draft in self.multiplexer.drafts.lock().await.values_mut() {
            draft.expired = true;
        }
    }

    pub(super) async fn cancel_directory_draft(&self, conversation: &ConversationRef) -> bool {
        let draft = self.multiplexer.drafts.lock().await.remove(conversation);
        if let Some(draft) = draft {
            self.revoke_action_group(&draft.id).await;
            true
        } else {
            false
        }
    }

    async fn attached_directory(&self, conversation: &ConversationRef) -> Option<String> {
        let current = self.sessions.current(conversation).await?;
        let mut cursor = None;
        let mut seen = std::collections::HashSet::new();
        loop {
            let page = self.agent.list_sessions(cursor, 100).await.ok()?;
            if let Some(session) = page.sessions.into_iter().find(|s| s.id == current) {
                return session.cwd;
            }
            cursor = page.next_cursor;
            if cursor.is_none() || !seen.insert(cursor.clone()) {
                return None;
            }
        }
    }

    pub(super) async fn begin_directory_draft(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        mutation: MultiplexerMutation,
    ) -> Result<(), EngineError> {
        self.cancel_directory_draft(conversation).await;
        self.tasks.take_input(conversation).await;
        self.interactions.take_session_input(conversation).await;
        self.interactions.take_reply_mode(conversation).await;
        let runtime = self
            .multiplexer
            .runtime(self.agent.as_ref(), conversation)
            .await
            .ok_or_else(|| EngineError::InvalidInput("workspace runtime is unavailable".into()))?;
        let home = runtime
            .resolve_directory("~", &runtime.default_directory())
            .await?;
        let (raw, source) = self
            .infer_directories(conversation, &mutation.target)
            .await?;
        let mut candidates = Vec::new();
        for path in raw {
            if !path.is_empty()
                && let Ok(path) = runtime.resolve_directory(&path, &home).await
                && !candidates.contains(&path)
            {
                candidates.push(path);
            }
        }
        let (directory, source) = if candidates.len() == 1 {
            (candidates[0].clone(), source.to_owned())
        } else {
            (
                home.clone(),
                if candidates.is_empty() {
                    "HOME fallback"
                } else {
                    "Multiple directories: choose a directory or use HOME"
                }
                .into(),
            )
        };
        let draft = DirectoryDraft {
            id: Uuid::new_v4().simple().to_string(),
            owner: owner.into(),
            generation: self.agent.generation(),
            workspace_generation: runtime.connection_generation(),
            epoch: self.sessions.epoch(conversation).await,
            backend: self.multiplexer_backend(conversation).await,
            mutation,
            browse_directory: directory.clone(),
            directory,
            home,
            source,
            candidates,
            awaiting_input: false,
            expired: false,
        };
        self.show_directory_preview(conversation, draft, None).await
    }

    async fn infer_directories(
        &self,
        conversation: &ConversationRef,
        target: &MultiplexerTarget,
    ) -> Result<(Vec<String>, &'static str), EngineError> {
        let snapshot = self.required_multiplexer_snapshot(conversation).await?;
        let current = self.sessions.current(conversation).await;
        Ok(match target {
            MultiplexerTarget::NewSession { .. } => (
                self.attached_directory(conversation)
                    .await
                    .into_iter()
                    .collect::<Vec<_>>(),
                "Attached agent",
            ),
            MultiplexerTarget::SplitPane { pane_id, .. } => {
                let pane = snapshot
                    .sessions
                    .iter()
                    .flat_map(|s| &s.windows)
                    .flat_map(|w| &w.panes)
                    .find(|p| &p.id == pane_id)
                    .ok_or_else(|| {
                        EngineError::InvalidInput("target pane no longer exists".into())
                    })?;
                (vec![pane.cwd.clone()], "Target pane")
            }
            MultiplexerTarget::NewWindow { session_id, .. } => {
                let session = snapshot
                    .sessions
                    .iter()
                    .find(|s| &s.id == session_id)
                    .ok_or_else(|| {
                        EngineError::InvalidInput("target session no longer exists".into())
                    })?;
                let attached = session
                    .windows
                    .iter()
                    .flat_map(|w| &w.panes)
                    .find(|p| current.is_some() && p.agent_session == current);
                if let Some(pane) = attached {
                    (vec![pane.cwd.clone()], "Attached pane")
                } else {
                    (
                        session
                            .windows
                            .iter()
                            .filter_map(|w| {
                                w.panes
                                    .iter()
                                    .find(|p| p.active)
                                    .or_else(|| w.panes.first())
                            })
                            .map(|p| p.cwd.clone())
                            .collect(),
                        "Active pane",
                    )
                }
            }
            MultiplexerTarget::ExistingPane { .. } => return Err(EngineError::InvalidAction),
        })
    }

    async fn directory_draft(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        id: &str,
    ) -> Result<DirectoryDraft, EngineError> {
        let draft = self
            .multiplexer
            .drafts
            .lock()
            .await
            .get(conversation)
            .cloned()
            .ok_or(EngineError::InvalidAction)?;
        if draft.id != id || draft.owner != owner {
            return Err(EngineError::InvalidAction);
        }
        let workspace_generation = self
            .multiplexer
            .runtime(self.agent.as_ref(), conversation)
            .await
            .map(crate::WorkspaceRuntimePort::connection_generation);
        if workspace_generation != Some(draft.workspace_generation)
            || draft.expired
            || draft.generation != self.agent.generation()
            || draft.epoch != self.sessions.epoch(conversation).await
            || draft.backend != self.multiplexer_backend(conversation).await
        {
            self.cancel_directory_draft(conversation).await;
            return Err(EngineError::InvalidAction);
        }
        Ok(draft)
    }

    async fn directory_view(
        &self,
        conversation: &ConversationRef,
        draft: DirectoryDraft,
        body: String,
        choices: Vec<(String, DirectoryAction, ActionStyle)>,
    ) -> Result<(), EngineError> {
        self.revoke_action_group(&draft.id).await;
        let mut view = OutboundView::text("Terminal · Directory", body);
        for (label, action, style) in choices {
            let token = self
                .issue_action(
                    conversation,
                    &draft.owner,
                    &draft.id,
                    UiAction::Multiplexer(
                        draft.backend,
                        MultiplexerUiAction::Directory {
                            id: draft.id.clone(),
                            action,
                        },
                    ),
                )
                .await;
            view.actions.push(ActionButton {
                label,
                token,
                style,
            });
        }
        self.multiplexer
            .drafts
            .lock()
            .await
            .insert(conversation.clone(), draft);
        self.send_view(conversation, &view).await?;
        Ok(())
    }

    async fn show_directory_preview(
        &self,
        conversation: &ConversationRef,
        mut draft: DirectoryDraft,
        error: Option<String>,
    ) -> Result<(), EngineError> {
        draft.awaiting_input = false;
        let operation = match &draft.mutation.target {
            MultiplexerTarget::NewSession { .. } => "New session".to_owned(),
            MultiplexerTarget::NewWindow { session_id, .. } => {
                format!("New window in {session_id}")
            }
            MultiplexerTarget::SplitPane {
                pane_id, direction, ..
            } => format!("Split {pane_id} · {direction:?}"),
            MultiplexerTarget::ExistingPane { .. } => return Err(EngineError::InvalidAction),
        };
        let mut body = format!(
            "{operation}\nDirectory: `{}`\nSource: {}\nCreate an interactive shell, then choose an agent.",
            draft.directory, draft.source
        );
        if let Some(error) = error {
            body.push_str(&format!("\n{error}"));
        }
        let mut choices = Vec::new();
        if draft.candidates.len() > 1 {
            choices.extend(draft.candidates.iter().map(|p| {
                (
                    p.clone(),
                    DirectoryAction::Select(p.clone()),
                    ActionStyle::Default,
                )
            }));
        }
        choices.extend([
            (
                "Create".into(),
                DirectoryAction::Confirm,
                ActionStyle::Primary,
            ),
            (
                "Choose directory".into(),
                DirectoryAction::Browse {
                    path: draft.directory.clone(),
                    page: 0,
                    hidden: false,
                },
                ActionStyle::Primary,
            ),
            (
                "Cancel".into(),
                DirectoryAction::Cancel,
                ActionStyle::Danger,
            ),
        ]);
        self.directory_view(conversation, draft, body, choices)
            .await
    }

    pub(super) async fn handle_directory_action(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        id: &str,
        action: DirectoryAction,
    ) -> Result<(), EngineError> {
        let mut draft = self.directory_draft(conversation, owner, id).await?;
        let runtime = self
            .multiplexer
            .runtime(self.agent.as_ref(), conversation)
            .await
            .ok_or_else(|| EngineError::InvalidInput("workspace runtime is unavailable".into()))?;
        match action {
            DirectoryAction::Cancel => {
                self.cancel_directory_draft(conversation).await;
                self.show_multiplexer_root(conversation, owner).await
            }
            DirectoryAction::Preview => {
                self.show_directory_preview(conversation, draft, None).await
            }
            DirectoryAction::Input => {
                draft.awaiting_input = true;
                let body = format!(
                    "Enter a directory path (absolute, ~, or relative to `{}`). Use /cancel to cancel.",
                    draft.browse_directory
                );
                self.directory_view(
                    conversation,
                    draft,
                    body,
                    Self::directory_back_actions().to_vec(),
                )
                .await
            }
            DirectoryAction::Select(path) => {
                match runtime.resolve_directory(&path, &draft.directory).await {
                    Ok(path) => {
                        draft.directory = path;
                        draft.source = "Selected directory".into();
                        draft.candidates.clear();
                        self.show_directory_preview(conversation, draft, None).await
                    }
                    Err(error) => {
                        self.show_directory_preview(conversation, draft, Some(error.to_string()))
                            .await
                    }
                }
            }
            DirectoryAction::Browse { path, page, hidden } => {
                self.browse_directory(conversation, draft, &path, page, hidden)
                    .await
            }
            DirectoryAction::Confirm => self.confirm_directory(conversation, owner, draft).await,
        }
    }

    async fn confirm_directory(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        mut draft: DirectoryDraft,
    ) -> Result<(), EngineError> {
        let runtime = self
            .multiplexer
            .runtime(self.agent.as_ref(), conversation)
            .await
            .ok_or_else(|| EngineError::InvalidInput("workspace runtime is unavailable".into()))?;
        let path = match runtime
            .resolve_directory(&draft.directory, &draft.home)
            .await
        {
            Ok(path) => path,
            Err(error) => {
                return self
                    .show_directory_preview(conversation, draft, Some(error.to_string()))
                    .await;
            }
        };
        let snapshot = self.required_multiplexer_snapshot(conversation).await?;
        match &mut draft.mutation.target {
            MultiplexerTarget::NewSession { cwd, .. } => *cwd = path,
            MultiplexerTarget::NewWindow {
                session_id, cwd, ..
            } => {
                if !snapshot.sessions.iter().any(|s| &s.id == session_id) {
                    return Err(EngineError::InvalidInput(
                        "target session no longer exists".into(),
                    ));
                }
                *cwd = path;
            }
            MultiplexerTarget::SplitPane { pane_id, cwd, .. } => {
                if !snapshot
                    .sessions
                    .iter()
                    .flat_map(|s| &s.windows)
                    .flat_map(|w| &w.panes)
                    .any(|p| &p.id == pane_id)
                {
                    return Err(EngineError::InvalidInput(
                        "target pane no longer exists".into(),
                    ));
                }
                *cwd = path;
            }
            MultiplexerTarget::ExistingPane { .. } => {
                return Err(EngineError::InvalidAction);
            }
        }
        self.cancel_directory_draft(conversation).await;
        self.execute_multiplexer_mutation(conversation, owner, draft.mutation)
            .await
    }

    fn directory_back_actions() -> [(String, DirectoryAction, ActionStyle); 2] {
        [
            (
                "Back".into(),
                DirectoryAction::Preview,
                ActionStyle::Primary,
            ),
            (
                "Cancel".into(),
                DirectoryAction::Cancel,
                ActionStyle::Danger,
            ),
        ]
    }

    async fn browse_directory(
        &self,
        conversation: &ConversationRef,
        mut draft: DirectoryDraft,
        path: &str,
        page: usize,
        hidden: bool,
    ) -> Result<(), EngineError> {
        let runtime = self
            .multiplexer
            .runtime(self.agent.as_ref(), conversation)
            .await
            .ok_or_else(|| EngineError::InvalidInput("workspace runtime is unavailable".into()))?;
        let listing = match runtime.list_directories(path, page, hidden).await {
            Ok(listing) => listing,
            Err(error) => {
                return self
                    .show_directory_preview(conversation, draft, Some(error.to_string()))
                    .await;
            }
        };
        draft.awaiting_input = false;
        draft.browse_directory.clone_from(&listing.directory);
        let mut choices = vec![(
            "Use this directory".into(),
            DirectoryAction::Select(listing.directory.clone()),
            ActionStyle::Primary,
        )];
        for entry in listing.entries {
            choices.push((
                entry.name,
                DirectoryAction::Browse {
                    path: entry.path,
                    page: 0,
                    hidden,
                },
                ActionStyle::Default,
            ));
        }
        if let Some(parent) = listing.parent {
            choices.push((
                "Parent".into(),
                DirectoryAction::Browse {
                    path: parent,
                    page: 0,
                    hidden,
                },
                ActionStyle::Primary,
            ));
        }
        choices.push((
            "HOME".into(),
            DirectoryAction::Browse {
                path: draft.home.clone(),
                page: 0,
                hidden,
            },
            ActionStyle::Primary,
        ));
        if listing.page > 0 {
            choices.push((
                "Previous".into(),
                DirectoryAction::Browse {
                    path: listing.directory.clone(),
                    page: listing.page - 1,
                    hidden,
                },
                ActionStyle::Primary,
            ));
        }
        if listing.page + 1 < listing.pages {
            choices.push((
                "Next".into(),
                DirectoryAction::Browse {
                    path: listing.directory.clone(),
                    page: listing.page + 1,
                    hidden,
                },
                ActionStyle::Primary,
            ));
        }
        choices.extend([
            (
                if hidden { "Hide hidden" } else { "Show hidden" }.into(),
                DirectoryAction::Browse {
                    path: listing.directory.clone(),
                    page: 0,
                    hidden: !hidden,
                },
                ActionStyle::Primary,
            ),
            (
                "Enter path".into(),
                DirectoryAction::Input,
                ActionStyle::Primary,
            ),
        ]);
        choices.extend(Self::directory_back_actions());
        let body = format!(
            "`{}`\nPage {} / {}",
            listing.directory,
            listing.page + 1,
            listing.pages
        );
        self.directory_view(conversation, draft, body, choices)
            .await
    }

    pub(super) async fn handle_directory_text(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        text: &str,
    ) -> Result<bool, EngineError> {
        let draft = self
            .multiplexer
            .drafts
            .lock()
            .await
            .get(conversation)
            .cloned();
        let Some(draft) = draft else {
            return Ok(false);
        };
        let mut draft = self.directory_draft(conversation, owner, &draft.id).await?;
        let parsed = super::parse_input(text);
        let command = matches!(parsed, Ok(ParsedInput::Command(_)))
            || (text.trim_start().starts_with('/')
                && matches!(parsed, Err(ref error) if !matches!(error, crate::InputParseError::UnknownCommand(_))));
        if command {
            self.cancel_directory_draft(conversation).await;
            if text.trim() == "/cancel" {
                self.send_view(
                    conversation,
                    &OutboundView::text("Terminal", "Creation cancelled."),
                )
                .await?;
                return Ok(true);
            }
            return Ok(false);
        }
        if !draft.awaiting_input {
            return Ok(false);
        }
        let runtime = self
            .multiplexer
            .runtime(self.agent.as_ref(), conversation)
            .await
            .ok_or_else(|| EngineError::InvalidInput("workspace runtime is unavailable".into()))?;
        match runtime
            .resolve_directory(text, &draft.browse_directory)
            .await
        {
            Ok(path) => {
                draft.directory = path;
                draft.source = "Entered directory".into();
                draft.candidates.clear();
                self.show_directory_preview(conversation, draft, None)
                    .await?;
            }
            Err(error) => {
                self.directory_view(
                    conversation,
                    draft,
                    format!("{error}\nEnter another path or /cancel."),
                    Self::directory_back_actions().to_vec(),
                )
                .await?;
            }
        }
        Ok(true)
    }
}
