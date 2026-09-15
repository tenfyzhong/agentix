use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::{Notify, oneshot};
use tokio::task::JoinSet;

use super::{Engine, SessionService};
use crate::{AgentAdapter, ChannelAdapter, ChannelCommand, ConversationRef, MultiplexerKind};

const MAX_MENU_WORKERS: usize = 128;
const MENU_FAST_PATH: Duration = Duration::from_millis(50);

#[derive(Default)]
pub(super) struct CommandMenus {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    entries: HashMap<ConversationRef, Entry>,
    workers: JoinSet<()>,
}

struct Entry {
    pending: Option<Request>,
    changed: Arc<Notify>,
}

struct Request {
    context: MenuContext,
    channel: Arc<dyn ChannelAdapter>,
    attached: bool,
    sync: bool,
    done: oneshot::Sender<()>,
}

struct MenuContext {
    agent: Arc<dyn AgentAdapter>,
    sessions: Arc<SessionService>,
    multiplexer_kind: MultiplexerKind,
    multiplexer_enabled: bool,
    has_tasks: bool,
}

impl CommandMenus {
    pub(super) fn abort(&self) {
        let mut state = self.state.lock().unwrap();
        state.workers.abort_all();
        state.entries.clear();
    }

    fn enqueue(&self, conversation: ConversationRef, mut request: Request) {
        let mut state = self.state.lock().unwrap();
        while state.workers.try_join_next().is_some() {}
        let skipped_sync = request.sync && !request.channel.supports_command_menu_sync();
        if let Some(entry) = state.entries.get_mut(&conversation) {
            if skipped_sync {
                request.sync = false;
            }
            entry.changed.notify_one();
            entry.changed = Arc::new(Notify::new());
            entry.pending = Some(request);
            return;
        }
        if skipped_sync {
            return;
        }
        if state.entries.len() >= MAX_MENU_WORKERS {
            tracing::warn!(?conversation, "command menu worker capacity reached");
            return;
        }
        state.entries.insert(
            conversation.clone(),
            Entry {
                pending: Some(request),
                changed: Arc::new(Notify::new()),
            },
        );
        state
            .workers
            .spawn(run(Arc::downgrade(&self.state), conversation));
    }
}

async fn run(state: Weak<Mutex<State>>, conversation: ConversationRef) {
    loop {
        let next = {
            let Some(state) = state.upgrade() else {
                return;
            };
            let mut state = state.lock().unwrap();
            let Some(entry) = state.entries.get_mut(&conversation) else {
                return;
            };
            if let Some(request) = entry.pending.take() {
                Some((request, entry.changed.clone()))
            } else {
                state.entries.remove(&conversation);
                None
            }
        };
        let Some((request, changed)) = next else {
            return;
        };
        // Discovery is read-only and can be superseded. Once publication starts,
        // retain ownership until it finishes, then publish the latest request.
        let menu = tokio::select! {
            biased;
            () = changed.notified() => continue,
            menu = request.context.build(&conversation, request.attached) => menu,
        };
        let result = if request.sync {
            request
                .channel
                .sync_command_menu(&conversation, &menu)
                .await
        } else {
            request.channel.set_command_menu(&conversation, &menu).await
        };
        if let Err(error) = result {
            tracing::warn!(%error, ?conversation, "failed to publish command menu");
        }
        let _ = request.done.send(());
    }
}

impl Engine {
    pub(super) async fn queue_command_menu(
        &self,
        conversation: &ConversationRef,
        attached: bool,
        sync: bool,
    ) {
        let Ok(channel) = self.channel(conversation.channel) else {
            return;
        };
        let (done, received) = oneshot::channel();
        self.menus.enqueue(
            conversation.clone(),
            Request {
                context: MenuContext {
                    agent: self.agent.clone(),
                    sessions: self.sessions.clone(),
                    multiplexer_kind: self.multiplexer_kind,
                    multiplexer_enabled: self.multiplexer_enabled,
                    has_tasks: self.tasks.backend.is_some(),
                },
                channel: channel.clone(),
                attached,
                sync,
                done,
            },
        );
        let _ = tokio::time::timeout(MENU_FAST_PATH, received).await;
    }
}

impl MenuContext {
    async fn build(&self, conversation: &ConversationRef, attached: bool) -> crate::CommandMenu {
        let mut menu = super::command_menu_for(
            attached && self.agent.capabilities().session_control,
            self.multiplexer_enabled.then_some(self.multiplexer_kind),
        );
        if attached && !self.agent.capabilities().session_control {
            menu.commands
                .push(ChannelCommand::new("last", "Show the latest turn again").contextual());
        }
        if attached && let Some(session) = self.sessions.current(conversation).await {
            let mut commands = Vec::new();
            for command in menu.commands.drain(..) {
                let session_command = crate::parse_input(&format!("/{}", command.name)).ok();
                if !matches!(
                    session_command,
                    Some(crate::ParsedInput::Command(crate::AgentCommand::Session(_)))
                ) || command.name == "exit"
                    || self.agent.supports_command(&session, &command.name).await
                {
                    commands.push(command);
                }
            }
            menu.commands = commands;
        }
        if attached
            && let Some(session) = self.sessions.current(conversation).await
            && !self.agent.session_access(&session).await.can_write()
        {
            menu.commands.retain(|command| {
                matches!(
                    command.name.as_str(),
                    "sessions"
                        | "rmux"
                        | "tmux"
                        | "current"
                        | "history"
                        | "last"
                        | "detach"
                        | "cancel"
                        | "help"
                )
            });
        }
        if self.has_tasks {
            menu.commands.push(ChannelCommand::new(
                "dashboard",
                "Browse projects and task boards",
            ));
            if attached {
                menu.commands.extend([
                    ChannelCommand::new("board", "Show this session's task board").contextual(),
                    ChannelCommand::new("jobs", "Browse this session's jobs").contextual(),
                    ChannelCommand::new("inboxes", "Browse this project's inbox").contextual(),
                    ChannelCommand::new("inbox", "Append a requirement to this project's inbox")
                        .contextual(),
                ]);
            }
        }
        menu.commands.sort_by(|left, right| {
            left.contextual
                .cmp(&right.contextual)
                .then_with(|| left.name.cmp(&right.name))
        });
        menu
    }
}
