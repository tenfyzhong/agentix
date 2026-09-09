use std::{collections::HashSet, path::Path};

use agentix_task::{InboxEntry, Project, WriteOptions};
use serde_json::json;

use super::browse::{PAGE_SIZE, escape, markdown_pages, page_count, short};
use super::{
    ConversationRef, EngineError, OutboundView, SessionId, TaskBoardView, TaskBrowse, error,
};

impl TaskBoardView<'_> {
    pub(in crate::engine) async fn show_current_inboxes(
        &self,
        conversation: &ConversationRef,
        owner: &str,
    ) -> Result<(), EngineError> {
        if let Some((_, project)) = self.inbox_project(conversation).await? {
            self.show_inboxes(conversation, owner, &project.id, 0)
                .await?;
        }
        Ok(())
    }

    pub(in crate::engine) async fn inbox_project(
        &self,
        conversation: &ConversationRef,
    ) -> Result<Option<(SessionId, Project)>, EngineError> {
        let Some(session) = self.require_board_session(conversation).await? else {
            return Ok(None);
        };
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut cwd = None;
        loop {
            let page = self.ui.agent().list_sessions(cursor, 100).await?;
            if let Some(summary) = page.sessions.iter().find(|s| s.id == session) {
                cwd = summary.cwd.clone();
                break;
            }
            cursor = page.next_cursor;
            if cursor.as_ref().is_none_or(|c| !seen.insert(c.clone())) {
                break;
            }
        }
        let project = self
            .tasks_service()?
            .project_for_session(cwd.as_deref().map(Path::new), Some(session.native_str()))
            .await
            .map_err(error)?;
        let Some(project) = project.filter(|p| p.archived_at.is_none()) else {
            self.ui.send_view(conversation, &OutboundView::text("Project inbox", "This session has no active registered project. Attach a session in a registered project directory.")).await?;
            return Ok(None);
        };
        // Session enumeration can await a remote adapter. Do not submit to a
        // directory resolved for an attachment that has since changed.
        if self.ui.sessions().current(conversation).await.as_ref() != Some(&session) {
            return Err(error("The attached session changed; retry the command."));
        }
        Ok(Some((session, project)))
    }

    pub(in crate::engine) async fn submit_inbox(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        event_id: &str,
        content: &str,
    ) -> Result<(), EngineError> {
        let Some((session, project)) = self.inbox_project(conversation).await? else {
            return Ok(());
        };
        let result = self
            .tasks_service()?
            .execute(
                json!({"command":"inbox.add","project":project.id,"content":content}),
                WriteOptions {
                    actor_ref: format!("im:{owner}"),
                    session_ref: Some(session.native_str().to_owned()),
                    idempotency_key: Some(format!(
                        "im:inbox:{}",
                        json!([conversation.channel, conversation.conversation_id, event_id])
                    )),
                    ..WriteOptions::default()
                },
            )
            .await
            .map_err(error)?;
        let entry: InboxEntry = serde_json::from_value(result.result).map_err(error)?;
        let mut view = OutboundView::text(
            "Inbox submission",
            format!(
                "**Project:** {}\n**Entry:** `{}`\n**Status:** {}\n\nAdded to the end of the project inbox.",
                escape(&project.name),
                entry.id,
                entry.status
            ),
        );
        if let Some(warning) = result.projection_pending {
            view.body.push_str(&format!(
                "\n\nSaved; document synchronization is pending: {}",
                escape(&warning)
            ));
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Inbox {
                id: entry.id.clone(),
                page: 0,
            },
            1,
            vec![
                (
                    "View inbox entry".into(),
                    TaskBrowse::Inbox {
                        id: entry.id,
                        page: 0,
                    },
                ),
                (
                    "Project inbox".into(),
                    TaskBrowse::Inboxes {
                        project: project.id,
                        page: 0,
                    },
                ),
            ],
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    pub(super) async fn show_inboxes(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        project: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        let result = service
            .execute(
                json!({"command":"inbox.list","project":project}),
                WriteOptions::default(),
            )
            .await
            .map_err(error)?;
        let entries: Vec<InboxEntry> = serde_json::from_value(result.result).map_err(error)?;
        let project = service
            .store()
            .project_result(project)
            .await
            .map_err(error)?;
        let pages = page_count(entries.len());
        let page = page.min(pages - 1);
        let mut view = OutboundView::text(
            "Project inbox",
            format!(
                "**Project:** {}\n**Entries ({})**\n\nUse /inbox <content> to append a requirement.",
                escape(&short(&project.name)),
                entries.len()
            ),
        );
        let mut buttons = Vec::new();
        for entry in entries.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE) {
            view.body.push_str(&format!(
                "\n\n**{}**\n{}",
                escape(&short(entry.title())),
                entry.status
            ));
            buttons.push((
                entry.title().into(),
                TaskBrowse::Inbox {
                    id: entry.id.clone(),
                    page: 0,
                },
            ));
        }
        if entries.is_empty() {
            view.body.push_str("\n\nNo inbox entries.");
        }
        if let Some(warning) = result.projection_pending {
            view.body.push_str(&format!(
                "\n\nDocument synchronization is pending: {}",
                escape(&warning)
            ));
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Inboxes {
                project: project.id.clone(),
                page,
            },
            pages,
            buttons,
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    pub(super) async fn show_inbox(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        id: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        let entry = service
            .store()
            .inbox_record(id)
            .await
            .map_err(error)?
            .ok_or_else(|| error("Inbox entry was removed."))?;
        service
            .execute(
                json!({"command":"inbox.sync","project":entry.project_id}),
                WriteOptions::default(),
            )
            .await
            .map_err(error)?;
        let entry = service
            .store()
            .inbox_record(id)
            .await
            .map_err(error)?
            .filter(|e| !e.deleted)
            .ok_or_else(|| error("Inbox entry was removed."))?;
        let project = service
            .store()
            .project_result(&entry.project_id)
            .await
            .map_err(error)?;
        let content = markdown_pages(&entry.content);
        let page = page.min(content.len() - 1);
        let mut view = OutboundView::text(
            short(entry.title()),
            format!(
                "**Entry:** `{}`\n**Project:** {}\n**Status:** {}\n\n{}",
                entry.id,
                escape(&short(&project.name)),
                entry.status,
                content[page]
            ),
        );
        let mut buttons = vec![(
            "Project inbox".into(),
            TaskBrowse::Inboxes {
                project: entry.project_id.clone(),
                page: 0,
            },
        )];
        if let Some(job) = &entry.job_id {
            buttons.push((
                "Job".into(),
                TaskBrowse::Job {
                    id: job.clone(),
                    page: 0,
                },
            ));
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Inbox {
                id: id.into(),
                page,
            },
            content.len(),
            buttons,
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }
}

impl TaskBoardView<'_> {
    pub(in crate::engine) async fn edit_inbox_message(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        original: &str,
        version: i64,
        text: &str,
    ) -> Result<(), EngineError> {
        let Some(service) = self.service.backend.as_ref() else {
            return Ok(());
        };
        let Ok(crate::ParsedInput::Command(crate::AgentCommand::Inbox(content))) =
            crate::parse_input(text)
        else {
            return Ok(());
        };
        let source =
            json!([conversation.channel, conversation.conversation_id, original]).to_string();
        let Some(id) = service
            .store()
            .inbox_id_by_source(&source)
            .await
            .map_err(error)?
        else {
            return Ok(());
        };
        let result = service.execute(json!({"command":"inbox.edit","inbox":id,"source":source,"version":version,"content":content}), WriteOptions { actor_ref:format!("im:{owner}"), ..WriteOptions::default() }).await.map_err(error)?;
        if let Some(warning) = result.projection_pending {
            return Err(error(format!(
                "Inbox saved; document synchronization is pending: {warning}"
            )));
        }
        Ok(())
    }
}

impl TaskBoardView<'_> {
    pub async fn poll_inbox_sources(&self) -> Result<Vec<crate::InboundEnvelope>, EngineError> {
        let Some(service) = self.service.backend.as_ref() else {
            return Ok(Vec::new());
        };
        let mut edits = Vec::new();
        for entry in service.store().inbox_sources().await.map_err(error)? {
            let Ok(source) =
                serde_json::from_str::<(crate::ChannelKind, String, String)>(&entry.source)
            else {
                continue;
            };
            let Some(channel) = self.ui.channels().get(&source.0) else {
                continue;
            };
            let reference =
                crate::MessageRef::new(ConversationRef::new(source.0, source.1), source.2);
            match channel.read_inbox_message(&reference).await {
                Ok(Some(envelope)) if matches!(&envelope.payload, crate::InboundPayload::TextEdited { version, .. } if *version > entry.source_version) =>
                {
                    edits.push(envelope);
                }
                Err(error) => tracing::warn!(%error, inbox = %entry.id, "Inbox source read failed"),
                _ => (),
            }
        }
        Ok(edits)
    }
}
