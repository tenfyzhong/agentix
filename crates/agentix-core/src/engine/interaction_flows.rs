//! Scoped IM actions and human reply workflows.
use super::{
    ActionButton, ActionScope, ActionStyle, ChannelKind, ConversationRef, DeliveryClass, Engine,
    EngineError, InputProgress, InteractionDecision, InteractionKey, InteractionKind,
    InteractionRequest, MessageRef, OutboundView, PendingInteractionView, SessionId, UiAction,
    Uuid, Value, ViewStatus, completed_input_body, decision_label, input_progress_body,
    input_questions, input_response, interaction_key, json,
};

impl Engine {
    pub(super) async fn cancel_pending_reply(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
    ) -> Result<(), EngineError> {
        self.tasks.take_input(conversation).await;
        self.interactions.take_session_input(conversation).await;
        if let Some(interaction) = self.interactions.take_reply_mode(conversation).await {
            self.show_input_question(conversation, owner_id, &interaction)
                .await?;
        }
        self.send_view(
            conversation,
            &OutboundView::text("Agentix", "Pending reply cancelled."),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn invalidate_session_actions(
        &self,
        session_id: &str,
    ) -> Result<(), EngineError> {
        let id = SessionId::new(session_id);
        self.interactions
            .actions
            .lock()
            .await
            .retain(|action| !action.targets_session(&id));
        self.interactions
            .pending
            .lock()
            .await
            .retain(|key, _| key.session_id != id);
        self.interactions
            .reply_modes
            .lock()
            .await
            .retain(|_, key| key.session_id != id);
        self.clear_session_stop_actions(&id).await?;
        Ok(())
    }

    pub(super) async fn render_interaction(
        &self,
        conversation: &ConversationRef,
        request: &InteractionRequest,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        let session_id = SessionId::new(&request.session_id);
        let interaction = interaction_key(request);
        let session_label = self.session_label(&session_id).await;
        let owner_id = self
            .interactions
            .owners
            .lock()
            .await
            .get(conversation)
            .cloned()
            .ok_or(EngineError::InvalidAction)?;
        let mut actions = Vec::new();
        let action_group = Uuid::new_v4().simple().to_string();
        let input = if request.kind == InteractionKind::UserInput {
            let questions = input_questions(request);
            let progress = InputProgress {
                answers: vec![None; questions.len()],
                questions,
                current: 0,
            };
            if delivery == DeliveryClass::Live {
                actions = self
                    .input_actions(
                        conversation,
                        &owner_id,
                        &action_group,
                        &interaction,
                        &progress,
                    )
                    .await;
            } else {
                let token = self
                    .issue_action(
                        conversation,
                        &owner_id,
                        &action_group,
                        UiAction::BeginInput(interaction.clone()),
                    )
                    .await;
                actions.push(ActionButton {
                    label: "Answer".into(),
                    token,
                    style: ActionStyle::Primary,
                });
            }
            Some(progress)
        } else {
            actions = self
                .approval_actions(
                    conversation,
                    &owner_id,
                    &action_group,
                    &interaction,
                    request,
                )
                .await;
            None
        };
        let view = OutboundView {
            title: format!("{} · {}", request.title, session_label),
            subtitle: Some(format!(
                "Turn {} · {}",
                request.turn_id,
                if delivery == DeliveryClass::Live {
                    "current session"
                } else {
                    "background session"
                }
            )),
            body: input.as_ref().map_or_else(
                || request.detail.clone(),
                |progress| input_progress_body(progress, false),
            ),
            status: ViewStatus::Waiting,
            actions,
        };
        let message = self.send_view(conversation, &view).await?;
        self.interactions.pending.lock().await.insert(
            interaction,
            PendingInteractionView {
                rpc_id: request.rpc_id.clone(),
                message,
                view,
                action_group,
                input,
            },
        );
        Ok(())
    }

    pub(super) async fn approval_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        group_id: &str,
        interaction: &InteractionKey,
        request: &InteractionRequest,
    ) -> Vec<ActionButton> {
        let mut actions = Vec::new();
        for decision in &request.available_decisions {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    group_id,
                    UiAction::Resolve {
                        interaction: interaction.clone(),
                        decision: InteractionDecision {
                            rpc_id: request.rpc_id.clone(),
                            response: json!({"decision": decision}),
                        },
                    },
                )
                .await;
            actions.push(ActionButton {
                label: decision_label(decision),
                token,
                style: if decision == "accept" || decision == "acceptForSession" {
                    ActionStyle::Primary
                } else if decision == "decline" {
                    ActionStyle::Danger
                } else {
                    ActionStyle::Default
                },
            });
        }
        actions
    }

    pub(super) async fn handle_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        token: &str,
        message: Option<&MessageRef>,
    ) -> Result<(), EngineError> {
        let tasks = self.tasks.view(self);
        let generation = self.agent.generation();
        let binding_epoch = self.sessions.epoch(conversation).await;
        let action = self
            .interactions
            .actions
            .lock()
            .await
            .consume(token, conversation, owner_id, generation, binding_epoch)
            .map_err(|_| EngineError::InvalidAction)?;
        self.disable_consumed_actions(conversation, message).await?;
        match action {
            UiAction::Task(action) => {
                tasks
                    .run_task_action(conversation, owner_id, action)
                    .await?;
            }
            UiAction::TaskBrowse(action) => {
                tasks.browse_tasks(conversation, owner_id, action).await?;
            }
            UiAction::Attach(session) => self.attach(conversation, owner_id, session).await?,
            UiAction::QueueControl { session_id, action } => {
                if self.sessions.current(conversation).await.as_ref() != Some(&session_id) {
                    return Err(EngineError::InvalidAction);
                }
                self.control_queue(conversation, &action).await?;
            }
            UiAction::Stop {
                session_id,
                turn_id,
            } => {
                self.turns
                    .stop_actions
                    .lock()
                    .await
                    .remove(&(session_id.clone(), turn_id.clone()));
                self.operations.stop(&session_id, &turn_id).await?;
            }
            UiAction::Resolve {
                interaction,
                decision,
            } => {
                let selected = decision
                    .response
                    .get("decision")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let pending = self.interactions.pending.lock().await.remove(&interaction);
                self.agent.resolve_interaction(decision).await?;
                if let Some(pending) = pending {
                    self.show_local_approval_resolution(conversation, pending, &selected)
                        .await?;
                }
            }
            UiAction::BeginInput(interaction) => {
                self.show_input_question(conversation, owner_id, &interaction)
                    .await?;
            }
            UiAction::SelectInput {
                interaction,
                answer,
            } => {
                self.answer_input(conversation, owner_id, &interaction, &answer)
                    .await?;
            }
            UiAction::BeginCustomInput(interaction) => {
                self.begin_custom_input(conversation, &interaction).await?;
            }
            UiAction::SessionCommand {
                session_id,
                command,
            } => {
                if self.current_session(conversation).await? != session_id {
                    return Err(EngineError::InvalidAction);
                }
                self.run_session_command(conversation, owner_id, command)
                    .await?;
            }
            UiAction::MultiplexerBackend(kind) => {
                self.rmux
                    .selected
                    .lock()
                    .await
                    .insert(conversation.clone(), kind);
                self.show_multiplexer_root(conversation, owner_id).await?;
            }
            UiAction::Multiplexer(backend, action) => {
                if self.multiplexer_backend(conversation).await != backend {
                    return Err(EngineError::InvalidAction);
                }
                self.handle_multiplexer_action(conversation, owner_id, action)
                    .await?;
            }
        }
        Ok(())
    }

    pub(super) async fn disable_consumed_actions(
        &self,
        conversation: &ConversationRef,
        message: Option<&MessageRef>,
    ) -> Result<(), EngineError> {
        if let Some(message) = message.filter(|message| &message.conversation == conversation)
            && let Err(error) = self
                .channel(conversation.channel)?
                .disable_actions(message)
                .await
        {
            tracing::warn!(%error, message = %message.message_id, "failed to disable consumed IM actions");
        }
        Ok(())
    }

    pub(super) async fn show_local_approval_resolution(
        &self,
        conversation: &ConversationRef,
        mut pending: PendingInteractionView,
        decision: &str,
    ) -> Result<(), EngineError> {
        let label = decision_label(decision);
        pending.view.body = if pending.view.body.is_empty() {
            format!("**Selected:** {label}")
        } else {
            format!("{}\n\n**Selected:** {label}", pending.view.body)
        };
        pending.view.status = if decision == "accept" || decision == "acceptForSession" {
            ViewStatus::Success
        } else {
            ViewStatus::Warning
        };
        pending.view.actions.clear();
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        Ok(())
    }

    pub(super) async fn input_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        group_id: &str,
        interaction: &InteractionKey,
        progress: &InputProgress,
    ) -> Vec<ActionButton> {
        let Some(question) = progress.questions.get(progress.current) else {
            return Vec::new();
        };
        let mut actions = Vec::with_capacity(question.options.len() + 1);
        for option in &question.options {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    group_id,
                    UiAction::SelectInput {
                        interaction: interaction.clone(),
                        answer: option.label.clone(),
                    },
                )
                .await;
            actions.push(ActionButton {
                label: option.label.clone(),
                token,
                style: ActionStyle::Primary,
            });
        }
        let token = self
            .issue_action(
                conversation,
                owner_id,
                group_id,
                UiAction::BeginCustomInput(interaction.clone()),
            )
            .await;
        actions.push(ActionButton {
            label: if question.options.is_empty() {
                "Answer…".into()
            } else {
                "Other…".into()
            },
            token,
            style: ActionStyle::Default,
        });
        actions
    }

    pub(super) async fn show_input_question(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        interaction: &InteractionKey,
    ) -> Result<(), EngineError> {
        let Some(mut pending) = self.interactions.pending.lock().await.remove(interaction) else {
            return Err(EngineError::InvalidAction);
        };
        let Some(progress) = pending.input.as_ref() else {
            return Err(EngineError::InvalidAction);
        };
        let action_group = Uuid::new_v4().simple().to_string();
        pending.view.body = input_progress_body(progress, false);
        pending.view.actions = self
            .input_actions(conversation, owner_id, &action_group, interaction, progress)
            .await;
        pending.view.status = ViewStatus::Waiting;
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        pending.action_group = action_group;
        self.interactions
            .pending
            .lock()
            .await
            .insert(interaction.clone(), pending);
        Ok(())
    }

    pub(super) async fn begin_custom_input(
        &self,
        conversation: &ConversationRef,
        interaction: &InteractionKey,
    ) -> Result<(), EngineError> {
        let Some(mut pending) = self.interactions.pending.lock().await.remove(interaction) else {
            return Err(EngineError::InvalidAction);
        };
        let Some(progress) = pending.input.as_ref() else {
            return Err(EngineError::InvalidAction);
        };
        pending.view.body = input_progress_body(progress, true);
        pending.view.actions.clear();
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        self.interactions
            .pending
            .lock()
            .await
            .insert(interaction.clone(), pending);
        self.interactions
            .reply_modes
            .lock()
            .await
            .insert(conversation.clone(), interaction.clone());
        Ok(())
    }

    pub(super) async fn answer_input(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        interaction: &InteractionKey,
        answer: &str,
    ) -> Result<(), EngineError> {
        let Some(mut pending) = self.interactions.pending.lock().await.remove(interaction) else {
            return Err(EngineError::InvalidAction);
        };
        let Some(progress) = pending.input.as_mut() else {
            return Err(EngineError::InvalidAction);
        };
        let Some(slot) = progress.answers.get_mut(progress.current) else {
            return Err(EngineError::InvalidAction);
        };
        *slot = Some(answer.to_owned());
        progress.current += 1;
        if progress.current < progress.questions.len() {
            let action_group = Uuid::new_v4().simple().to_string();
            pending.view.body = input_progress_body(progress, false);
            pending.view.actions = self
                .input_actions(conversation, owner_id, &action_group, interaction, progress)
                .await;
            self.channel(conversation.channel)?
                .update(conversation, &pending.message, &pending.view)
                .await?;
            pending.action_group = action_group;
            self.interactions
                .pending
                .lock()
                .await
                .insert(interaction.clone(), pending);
            return Ok(());
        }

        let response = input_response(progress);
        self.agent
            .resolve_interaction(InteractionDecision {
                rpc_id: pending.rpc_id.clone(),
                response,
            })
            .await?;
        pending.view.body = completed_input_body(progress);
        pending.view.status = ViewStatus::Success;
        pending.view.actions.clear();
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        Ok(())
    }

    pub(super) async fn resolve_external_interaction(
        &self,
        conversation: &ConversationRef,
        interaction: &InteractionKey,
    ) -> Result<(), EngineError> {
        let Some(mut pending) = self.interactions.pending.lock().await.remove(interaction) else {
            return Ok(());
        };
        self.revoke_action_group(&pending.action_group).await;
        self.interactions
            .reply_modes
            .lock()
            .await
            .retain(|_, pending| pending != interaction);
        let channel_name = match conversation.channel {
            ChannelKind::Telegram => "Telegram",
            ChannelKind::Feishu => "Feishu",
        };
        let resolution = format!("**Resolved:** Outside {channel_name}");
        pending.view.body = if pending.view.body.is_empty() {
            resolution
        } else {
            format!("{}\n\n{resolution}", pending.view.body)
        };
        pending.view.status = ViewStatus::Muted;
        pending.view.actions.clear();
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        Ok(())
    }

    pub(super) async fn resolve_external_request(
        &self,
        conversation: &ConversationRef,
        session_id: SessionId,
        request_id: String,
    ) -> Result<(), EngineError> {
        self.resolve_external_interaction(
            conversation,
            &InteractionKey {
                session_id,
                request_id,
            },
        )
        .await
    }

    pub(super) async fn issue_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        group_id: &str,
        action: UiAction,
    ) -> String {
        let generation = self.agent.generation();
        let binding_epoch = self.sessions.epoch(conversation).await;
        self.interactions.actions.lock().await.issue(
            ActionScope::new(
                conversation.clone(),
                owner_id,
                generation,
                binding_epoch,
                group_id,
            ),
            action,
        )
    }

    pub(super) async fn revoke_action_group(&self, group_id: &str) {
        self.interactions
            .actions
            .lock()
            .await
            .revoke_group(group_id);
    }
}
