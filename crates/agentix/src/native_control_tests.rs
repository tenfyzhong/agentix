//! Production control listener/handler/registry and real Node plugin runtimes.
//! Only the native host API and Claude MCP client are fixtures; no model network.
use super::*;
use agentix_core::{AgentKind, AgentRegistry, SessionCommand, SessionId, SessionOperation};
use tokio::io::{AsyncBufReadExt, BufReader};

// Each stack launches three Node processes. Bound independent fixtures so OS
// process contention does not consume the isolation tests' request deadlines.
// Requests and all three backends within each fixture remain concurrent.
static NATIVE_STACKS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

struct Stack {
    directory: tempfile::TempDir,
    endpoint: String,
    agent: Arc<dyn AgentAdapter>,
    hosts: Vec<tokio::process::Child>,
    shutdown: CancellationToken,
    handler_shutdown: CancellationToken,
    listener: tokio::task::JoinHandle<Result<()>>,
    handler: tokio::task::JoinHandle<()>,
    _permit: tokio::sync::SemaphorePermit<'static>,
}

impl Stack {
    async fn start() -> Self {
        let permit = NATIVE_STACKS.acquire().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let git = tokio::process::Command::new("git")
            .args(["init", "--quiet", "--initial-branch=fixture"])
            .arg(directory.path())
            .status()
            .await
            .unwrap();
        assert!(git.success());
        let git = tokio::process::Command::new("git")
            .current_dir(directory.path())
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-s",
                "--allow-empty",
                "--quiet",
                "-m",
                "Fixture",
            ])
            .status()
            .await
            .unwrap();
        assert!(git.success());
        let endpoint = format!("unix://{}", directory.path().join("control.sock").display());
        let hub = Arc::new(BridgeHub::new());
        let agents = [AgentKind::Pi, AgentKind::Omp, AgentKind::Claude]
            .into_iter()
            .map(|kind| {
                let adapter: Arc<dyn AgentAdapter> =
                    Arc::new(BridgeAdapter::new(kind, hub.clone(), directory.path()));
                (kind, adapter)
            })
            .collect();
        let agent: Arc<dyn AgentAdapter> = Arc::new(AgentRegistry::new(agents).unwrap());
        let config = directory.path().join("agentix.toml");
        std::fs::write(
            &config,
            format!("[agent.pi]\nsession_dir='{}'\n[storage]\npath='{}'\n[channel]\nkind='telegram'\n[channel.telegram]\ntoken='mock-token'\n", directory.path().display(), directory.path().join("state.sqlite3").display()),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel(16);
        let shutdown = CancellationToken::new();
        let listener = tokio::spawn({
            let endpoint = endpoint.clone();
            let shutdown = shutdown.clone();
            async move { control::serve_with_bridge(&endpoint, tx, shutdown, Some(hub)).await }
        });
        let handler_shutdown = shutdown.child_token();
        let handler = tokio::spawn(run_control_handler(
            rx,
            agent.clone(),
            None,
            Arc::new(ClaimRegistry::default()),
            config,
            handler_shutdown.clone(),
        ));
        let mut stack = Self {
            directory,
            endpoint,
            agent,
            hosts: vec![],
            shutdown,
            handler_shutdown,
            listener,
            handler,
            _permit: permit,
        };
        for kind in [AgentKind::Pi, AgentKind::Omp, AgentKind::Claude] {
            stack.spawn_host(kind).await;
        }
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(page) = stack.list(None, 10).await
                    && page["sessions"].as_array().unwrap().len() == 3
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("all three production plugins register");
        stack
    }

    async fn spawn_host(&mut self, kind: AgentKind) {
        let claude = kind == AgentKind::Claude;
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(if claude {
            "../../plugins/agentix-bridge/tests/claude-host.mjs"
        } else {
            "../../plugins/agentix-bridge/tests/host.mjs"
        });
        let mut command = tokio::process::Command::new("node");
        command.arg(fixture).arg(&self.endpoint);
        if !claude {
            command.arg(kind.as_str());
        }
        command.arg(self.directory.path());
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(20),
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(line.trim(), "ready");
        self.hosts.push(child);
    }

    async fn request(&self, request: control::ControlRequest) -> Result<Value> {
        tokio::time::timeout(
            Duration::from_secs(10),
            control::request(&self.endpoint, &request),
        )
        .await
        .unwrap()
    }

    async fn list(&self, cursor: Option<String>, limit: u32) -> Result<Value> {
        self.request(control::ControlRequest::Sessions { cursor, limit })
            .await
    }

    async fn operation(&self, operation: SessionOperation) -> Result<Value> {
        self.request(control::ControlRequest::Session(operation))
            .await
    }

    async fn command(&self, session: &SessionId, command: SessionCommand) -> Result<Value> {
        self.operation(SessionOperation::Command {
            session: session.clone(),
            command,
        })
        .await
    }

    async fn send(&self, session: &SessionId, text: &str, turn: Option<String>) -> Result<Value> {
        self.operation(SessionOperation::Send {
            session: session.clone(),
            text: text.into(),
            expected_turn: turn,
        })
        .await
    }

    async fn history(&self, session: &SessionId) -> Value {
        self.operation(SessionOperation::History {
            session: session.clone(),
            cursor: None,
            limit: 20,
        })
        .await
        .unwrap()
    }

    async fn completed(&self, session: &SessionId, turn: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let history = self.history(session).await;
                if let Some(found) = history["turns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|t| t["id"] == turn && t["status"] != "inProgress")
                {
                    return found.clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("host completes the requested turn")
    }

    async fn close(mut self) {
        for host in &mut self.hosts {
            assert!(
                host.try_wait().unwrap().is_none(),
                "control operations preserve native host"
            );
            let status = tokio::process::Command::new("kill")
                .args(["-TERM", &host.id().unwrap().to_string()])
                .status()
                .await
                .unwrap();
            assert!(status.success());
            tokio::time::timeout(Duration::from_secs(5), host.wait())
                .await
                .unwrap()
                .unwrap();
        }
        self.shutdown.cancel();
        self.listener.await.unwrap().unwrap();
        self.handler.await.unwrap();
        assert!(!self.directory.path().join("control.sock").exists());
    }
}

#[tokio::test]
async fn native_control_routes_all_hosts_and_paginates_without_crossing_sessions() {
    let stack = Stack::start().await;
    let mut ids = vec![];
    let mut cursor = None;
    loop {
        let page = stack.list(cursor, 1).await.unwrap();
        assert_eq!(page["sessions"].as_array().unwrap().len(), 1);
        ids.push(page["sessions"][0]["id"].as_str().unwrap().to_owned());
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
        assert!(ids.len() < 4, "pagination must advance");
    }
    ids.sort();
    assert_eq!(
        ids,
        ["claude:native-claude", "omp:native-id", "pi:native-id"]
    );
    for id in &ids {
        let session = SessionId::new(id);
        assert!(
            stack.history(&session).await["turns"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let text = format!("hello {id}");
        let response = stack.send(&session, &text, None).await.unwrap();
        let turn = response["turn_id"].as_str().unwrap();
        let history = stack.completed(&session, turn).await;
        assert_eq!(history["user_text"], text);
        assert_eq!(
            history["agent_text"],
            if id.starts_with("claude:") {
                "Claude reply"
            } else {
                "bridge answer"
            }
        );
        assert_eq!(
            stack.history(&session).await["turns"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let status = stack
            .command(&session, SessionCommand::Status)
            .await
            .unwrap();
        assert!(status["body"].as_str().unwrap().contains("Session:"));
        assert!(
            stack
                .send(&session, "  ", None)
                .await
                .unwrap_err()
                .to_string()
                .contains("nonempty")
        );
    }
    for id in &ids {
        assert_eq!(
            stack.history(&SessionId::new(id)).await["turns"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    let error = stack
        .send(&SessionId::new("pi:missing"), "hello", None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unavailable"));
    let error = stack
        .request(control::ControlRequest::Call {
            method: "debug/echo".into(),
            params: json!({}),
        })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("only available for the Codex backend")
    );
    stack.close().await;
}

#[tokio::test]
async fn native_control_commands_and_capability_rejections_cross_the_real_plugins() {
    let stack = Stack::start().await;
    for prefix in ["pi", "omp", "claude"] {
        let claude = prefix == "claude";
        let session = SessionId::new(if claude {
            "claude:native-claude".into()
        } else {
            format!("{prefix}:native-id")
        });
        for command in [
            SessionCommand::Model(None),
            SessionCommand::Model(Some("openai/test".into())),
            SessionCommand::Reasoning(None),
            SessionCommand::Reasoning(Some("high".into())),
            SessionCommand::Rename(Some("Renamed".into())),
            SessionCommand::Compact,
            SessionCommand::Skills,
            SessionCommand::Diff,
        ] {
            let result = stack.command(&session, command).await;
            if claude {
                assert!(result.unwrap_err().to_string().contains("not supported"));
            } else {
                assert!(!result.unwrap()["body"].as_str().unwrap().is_empty());
            }
        }
        for command in [
            SessionCommand::Fork,
            SessionCommand::Fast(None),
            SessionCommand::Clear(None),
            SessionCommand::Exit,
            SessionCommand::Plan {
                enabled: true,
                prompt: None,
            },
            SessionCommand::Goal(agentix_core::GoalCommand::Show),
            SessionCommand::Review,
            SessionCommand::Mcp,
        ] {
            assert!(
                stack
                    .command(&session, command)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("not supported")
            );
        }
        let stopped = stack
            .operation(SessionOperation::Stop {
                session: session.clone(),
                turn: "missing".into(),
            })
            .await;
        if claude {
            assert!(stopped.unwrap_err().to_string().contains("not supported"));
        } else {
            stopped.unwrap();
        }
        let error = stack
            .send(&session, "steer", Some("missing".into()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains(if claude {
            "not supported"
        } else {
            "No active turn"
        }));
        let queue = stack.agent.queued_prompts().unwrap();
        assert!(
            queue
                .list_queued_prompts(&session)
                .await
                .unwrap()
                .is_empty()
        );
        if !claude {
            let started = stack.send(&session, "hold", None).await.unwrap();
            let turn = started["turn_id"].as_str().unwrap();
            let steered = stack
                .send(&session, "steered", Some(turn.into()))
                .await
                .unwrap();
            assert_eq!(steered["turn_id"], turn);
            assert_eq!(stack.completed(&session, turn).await["status"], "completed");
        }
        queue.control_queue(&session, "clear").await.unwrap();
        queue.control_queue(&session, "resume").await.unwrap();
        assert!(queue.queue_status(&session).await.unwrap().is_some());
    }
    stack.close().await;
}

#[tokio::test]
async fn native_control_stop_and_queue_preserve_host_and_backend_ownership() {
    let stack = Stack::start().await;
    for prefix in ["pi", "omp"] {
        let session = SessionId::new(format!("{prefix}:native-id"));
        let started = stack.send(&session, "hold", None).await.unwrap();
        let turn = started["turn_id"].as_str().unwrap();
        assert!(
            stack
                .send(&session, "busy", None)
                .await
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        let queue = stack.agent.queued_prompts().unwrap();
        let first = queue
            .queue_prompt(&session, "queued", "shared-receipt")
            .await
            .unwrap();
        let again = queue
            .queue_prompt(&session, "queued", "shared-receipt")
            .await
            .unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(queue.list_queued_prompts(&session).await.unwrap().len(), 1);
        stack
            .operation(SessionOperation::Stop {
                session: session.clone(),
                turn: turn.into(),
            })
            .await
            .unwrap();
        assert_eq!(
            stack.completed(&session, turn).await["status"],
            "interrupted"
        );
        assert!(
            queue
                .queue_status(&session)
                .await
                .unwrap()
                .unwrap()
                .contains("Paused")
        );
        assert_eq!(queue.list_queued_prompts(&session).await.unwrap().len(), 1);
        queue.control_queue(&session, "resume").await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let history = stack.history(&session).await;
                if history["turns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["user_text"] == "queued" && t["status"] == "completed")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            queue
                .list_queued_prompts(&session)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        stack.history(&SessionId::new("claude:native-claude")).await["turns"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    stack.close().await;
}

#[tokio::test]
async fn native_control_history_cursors_preserve_turn_ids_across_all_hosts() {
    let stack = Stack::start().await;
    for id in ["pi:native-id", "omp:native-id", "claude:native-claude"] {
        let session = SessionId::new(id);
        let mut turns = Vec::new();
        for text in ["first", "second"] {
            let result = stack.send(&session, text, None).await.unwrap();
            let turn = result["turn_id"].as_str().unwrap().to_owned();
            stack.completed(&session, &turn).await;
            turns.push(turn);
        }
        let recent = stack
            .operation(SessionOperation::History {
                session: session.clone(),
                cursor: None,
                limit: 1,
            })
            .await
            .unwrap();
        assert_eq!(recent["turns"].as_array().unwrap().len(), 1);
        assert_eq!(recent["turns"][0]["id"], turns[1]);
        let older = stack
            .operation(SessionOperation::History {
                session: session.clone(),
                cursor: Some(recent["older_cursor"].as_str().unwrap().into()),
                limit: 1,
            })
            .await
            .unwrap();
        assert_eq!(older["turns"].as_array().unwrap().len(), 1);
        assert_eq!(older["turns"][0]["id"], turns[0]);
        assert!(older["older_cursor"].is_null());
        let newer = stack
            .operation(SessionOperation::History {
                session: session.clone(),
                cursor: Some(older["newer_cursor"].as_str().unwrap().into()),
                limit: 1,
            })
            .await
            .unwrap();
        assert_eq!(newer["turns"][0]["id"], turns[1]);
        assert!(
            stack
                .operation(SessionOperation::History {
                    session,
                    cursor: Some("invalid".into()),
                    limit: 1
                })
                .await
                .unwrap_err()
                .to_string()
                .contains("cursor")
        );
    }
    stack.close().await;
}

#[tokio::test]
async fn native_control_claims_and_disconnected_host_errors_leave_service_available() {
    let mut stack = Stack::start().await;
    let claimed = stack
        .request(control::ControlRequest::Claim { ttl_minutes: 5 })
        .await
        .unwrap();
    assert!(claimed["command"].as_str().unwrap().starts_with("/claim "));
    assert!(claimed["expiresAt"].as_u64().unwrap() > unix_timestamp().unwrap());
    assert!(
        stack
            .request(control::ControlRequest::Claim { ttl_minutes: 0 })
            .await
            .is_err()
    );
    let mut host = stack.hosts.remove(0);
    host.kill().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if stack.list(None, 10).await.unwrap()["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["id"] != "pi:native-id")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let session = SessionId::new("pi:native-id");
    assert!(
        stack
            .send(&session, "offline", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    stack.spawn_host(AgentKind::Pi).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while stack.list(None, 10).await.unwrap()["sessions"]
            .as_array()
            .unwrap()
            .len()
            != 3
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let turn = stack.send(&session, "reconnected", None).await.unwrap();
    assert_eq!(
        stack
            .completed(&session, turn["turn_id"].as_str().unwrap())
            .await["agent_text"],
        "bridge answer"
    );
    stack.close().await;
}

#[tokio::test]
async fn slow_native_control_ack_does_not_block_reads_stop_or_other_hosts() {
    let stack = Stack::start().await;
    let endpoint = stack.endpoint.clone();
    let slow = tokio::spawn(async move {
        control::request(
            &endpoint,
            &control::ControlRequest::Session(SessionOperation::Send {
                session: SessionId::new("pi:native-id"),
                text: "wait-for-test-ack".into(),
                expected_turn: None,
            }),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !stack.directory.path().join("pi.prompt-waiting").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("native Pi received the request and is withholding its acknowledgment");
    let independent = tokio::time::timeout(Duration::from_secs(2), async {
        let sent = stack
            .send(&SessionId::new("omp:native-id"), "independent", None)
            .await
            .unwrap();
        assert!(sent["turn_id"].is_string());
        assert_eq!(
            stack.list(None, 10).await.unwrap()["sessions"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let pi = SessionId::new("pi:native-id");
        let history = stack.history(&pi).await;
        let turn = history["turns"].as_array().unwrap().last().unwrap()["id"]
            .as_str()
            .unwrap();
        stack
            .operation(SessionOperation::Stop {
                session: pi,
                turn: turn.into(),
            })
            .await
            .unwrap();
    })
    .await;
    assert!(
        !slow.is_finished(),
        "the slow request must still be waiting during the other requests"
    );
    std::fs::write(stack.directory.path().join("pi.prompt-release"), "release").unwrap();
    tokio::time::timeout(Duration::from_secs(5), slow)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    stack.close().await;
    independent.expect("a slow native acknowledgment must not block independent control clients");
}

#[tokio::test]
async fn control_dispatch_shutdown_cancels_a_waiting_host_without_waiting_for_its_ack() {
    let stack = Stack::start().await;
    let endpoint = stack.endpoint.clone();
    let slow = tokio::spawn(async move {
        control::request(
            &endpoint,
            &control::ControlRequest::Session(SessionOperation::Send {
                session: SessionId::new("pi:native-id"),
                text: "wait-for-test-ack".into(),
                expected_turn: None,
            }),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !stack.directory.path().join("pi.prompt-waiting").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    stack.handler_shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !stack.handler.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("handler cancels workers independently of listener or host shutdown");
    assert!(slow.await.unwrap().is_err());
    assert!(!stack.directory.path().join("pi.prompt-release").exists());
    stack.close().await;
}
