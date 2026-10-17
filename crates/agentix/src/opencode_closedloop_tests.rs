//! Real Telegram adapter, Engine, Registry, control listener, Bridge and `OpenCode`.
//! Only Telegram's HTTP service and the model provider are deterministic fixtures.
use super::*;
use crate::telegram_test_support as telegram_support;
use agentix_core::{AgentKind, AgentRegistry, SqliteState};
use telegram_support::MockTelegramApi;
use teloxide::Bot;
use tokio::io::{AsyncBufReadExt, BufReader};

struct NativeHost {
    child: tokio::process::Child,
    socket: PathBuf,
    pid_file: PathBuf,
    rmux: bool,
}
impl Drop for NativeHost {
    fn drop(&mut self) {
        let command = || {
            let mut command = std::process::Command::new(if self.rmux {
                std::env::var("AGENTIX_TEST_RMUX").unwrap_or_else(|_| "rmux".into())
            } else {
                std::env::var("AGENTIX_TEST_TMUX").unwrap_or_else(|_| "tmux".into())
            });
            if !self.rmux {
                command.arg("-S").arg(&self.socket);
            }
            command
        };
        // OpenCode 2 can survive a terminal's SIGHUP. Derive ownership from the
        // isolated server's panes, then kill only those pane process trees.
        let mut owned = std::collections::HashSet::new();
        let mut ordered = Vec::new();
        if let Ok(markers) = std::fs::read_to_string(&self.pid_file) {
            for line in markers.lines() {
                let Some((pid, started)) = line.split_once('\t') else {
                    continue;
                };
                let Ok(pid) = pid.parse::<i32>() else {
                    continue;
                };
                let Ok(current) = std::process::Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "lstart="])
                    .env("LC_ALL", "C")
                    .output()
                else {
                    continue;
                };
                // A PID from our trusted launcher is valid only for the same OS
                // process generation, preventing cleanup of a recycled PID.
                if current.status.success()
                    && String::from_utf8_lossy(&current.stdout).trim() == started.trim()
                    && owned.insert(pid)
                {
                    ordered.push(pid);
                }
            }
        }
        if let Ok(output) = command()
            .args(["list-panes", "-a", "-F", "#{pane_pid}"])
            .output()
        {
            for pid in String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<i32>().ok())
            {
                if owned.insert(pid) {
                    ordered.push(pid);
                }
            }
        }
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-Ao", "pid=,ppid="])
            .output()
        {
            let pairs: Vec<_> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let mut values = line.split_whitespace();
                    Some((
                        values.next()?.parse::<i32>().ok()?,
                        values.next()?.parse::<i32>().ok()?,
                    ))
                })
                .collect();
            loop {
                let before = owned.len();
                for (pid, parent) in &pairs {
                    if owned.contains(parent) && owned.insert(*pid) {
                        ordered.push(*pid);
                    }
                }
                if before == owned.len() {
                    break;
                }
            }
        }
        // Parents first: the TUI cannot respawn its private server during cleanup.
        for pid in ordered {
            if pid > 1 {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
        let _ = command().arg("kill-server").output();
        let _ = self.child.start_kill();
    }
}

fn message(id: u64, text: &str) -> Value {
    serde_json::json!({"update_id":id,"message":{"message_id":id,"date":1,"chat":{"id":42,"type":"private","first_name":"Owner"},"from":{"id":42,"is_bot":false,"first_name":"Owner"},"text":text}})
}
fn callback(id: u64, token: &str) -> Value {
    serde_json::json!({"update_id":id,"callback_query":{"id":format!("cb-{id}"),"from":{"id":42,"is_bot":false,"first_name":"Owner"},"chat_instance":"native","message":{"message_id":77,"date":1,"chat":{"id":42,"type":"private","first_name":"Owner"},"text":"Native"},"data":token}})
}
async fn wait_body(api: &MockTelegramApi, needle: &str, after: usize) -> Value {
    let result = tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            for request in api.requests().await.into_iter().skip(after) {
                let body: Value = serde_json::from_str(&request.body).unwrap_or_default();
                if request.body.contains(needle) {
                    return body;
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await;
    result.unwrap_or_else(|_| panic!("Telegram never received {needle}"))
}
async fn wait_final(api: &MockTelegramApi, needle: &str, after: usize) {
    tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            if api.requests().await.iter().skip(after).any(|request| {
                request.target.to_ascii_lowercase().contains("sendmessage")
                    && request.body.contains(needle)
                    && request.body.contains("Completed")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Missing fresh completed Telegram card: {needle}"));
}
fn button(body: &Value, needle: &str) -> String {
    body["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r.as_array().unwrap())
        .find(|b| b["text"].as_str().unwrap().contains(needle))
        .unwrap_or_else(|| panic!("missing {needle}: {body}"))
        .get("callback_data")
        .unwrap()
        .as_str()
        .unwrap()
        .into()
}

#[tokio::test]
#[ignore = "requires native OpenCode 2.0.32, Node, Taskix and tmux"]
#[allow(clippy::too_many_lines)] // One sequential IM creation, native interaction and service recovery scenario.
async fn opencode_native_telegram_engine_permission_and_multiselect_closed_loop() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = format!("unix://{}", directory.path().join("control.sock").display());
    let hub = Arc::new(BridgeHub::new());
    let socket = directory.path().join("tmux.sock");
    let rmux = std::env::var("AGENTIX_TEST_MULTIPLEXER").as_deref() == Ok("rmux");
    let kind = if rmux {
        agentix_core::MultiplexerKind::Rmux
    } else {
        agentix_core::MultiplexerKind::Tmux
    };
    let driver: Arc<dyn agentix_multiplexer::MultiplexerDriver> = if rmux {
        Arc::new(agentix_rmux::RmuxDriver)
    } else {
        Arc::new(agentix_tmux::TmuxDriver::with_command(
            std::env::var("AGENTIX_TEST_TMUX")
                .unwrap_or_else(|_| "tmux".into())
                .into(),
            Some(socket.clone()),
        ))
    };
    let native: Arc<dyn AgentAdapter> = Arc::new(
        BridgeAdapter::new(
            AgentKind::OpenCode,
            hub.clone(),
            directory.path().join("bridge"),
        )
        .with_workspace(
            &directory.path().join("launch"),
            vec![],
            &directory.path().join("project"),
        )
        .with_multiplexer(driver),
    );
    let agent: Arc<dyn AgentAdapter> =
        Arc::new(AgentRegistry::new(vec![(AgentKind::OpenCode, native)]).unwrap());
    let (control_tx, _control_rx) = mpsc::channel(16);
    let shutdown = CancellationToken::new();
    let listener = tokio::spawn({
        let endpoint = endpoint.clone();
        let shutdown = shutdown.clone();
        let hub = hub.clone();
        async move { control::serve_with_bridge(&endpoint, control_tx, shutdown, Some(hub)).await }
    });
    let child = tokio::process::Command::new("node")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .arg("plugins/agentix-bridge/tests/opencode-closedloop-host.mjs")
        .arg(directory.path())
        .arg(&endpoint)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut host = NativeHost {
        child,
        socket,
        pid_file: directory.path().join("native-pids"),
        rmux,
    };
    let mut ready = String::new();
    tokio::time::timeout(
        Duration::from_secs(90),
        BufReader::new(host.child.stdout.take().unwrap()).read_line(&mut ready),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ready.trim(), "ready");
    let telegram = MockTelegramApi::start().await;
    let channel = Arc::new(agentix_telegram::TelegramAdapter::with_bot(
        Bot::new("fixture-token").set_api_url(telegram.api_url().parse().unwrap()),
        agentix_telegram::TelegramPolicy::new([42]),
    ));
    let state = SqliteState::open(&directory.path().join("agentix.sqlite3"))
        .await
        .unwrap();
    let engine = Arc::new(
        Engine::new(agent.clone(), state.clone(), vec![channel.clone()])
            .with_multiplexer_kind(kind),
    );
    engine.restore_bindings_deferred().await.unwrap();
    let (tx, rx) = mpsc::channel(32);
    let channel_task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { channel.run(tx, shutdown).await }
    });
    let engine_task = tokio::spawn(run_engine_loop(engine.clone(), agent, rx, shutdown.clone()));
    telegram
        .push_updates(vec![message(90, &format!("/{kind} opencode"))])
        .await;
    let menu = wait_body(&telegram, "+ Session", 0).await;
    let before = telegram.requests().await.len();
    telegram
        .push_updates(vec![callback(91, &button(&menu, "+ Session"))])
        .await;
    let create = wait_body(&telegram, "Create", before).await;
    let before = telegram.requests().await.len();
    telegram
        .push_updates(vec![callback(92, &button(&create, "Create"))])
        .await;
    let picker = wait_body(&telegram, "OpenCode", before).await;
    let before = telegram.requests().await.len();
    telegram
        .push_updates(vec![callback(93, &button(&picker, "OpenCode"))])
        .await;
    wait_body(&telegram, "Attached", before).await;
    let before = telegram.requests().await.len();
    telegram
        .push_updates(vec![message(101, "plain-loop")])
        .await;
    wait_final(&telegram, "Closed loop plain answer", before).await;
    let before = telegram.requests().await.len();
    telegram
        .push_updates(vec![message(102, "permission-loop")])
        .await;
    let permission = wait_body(&telegram, "OpenCode permission", before).await;
    let old_token = button(&permission, "Allow once");
    // Restart the production service components while the native tool remains
    // blocked, retaining only SQLite and the live terminal process.
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), engine_task)
        .await
        .unwrap()
        .unwrap();
    channel_task.abort();
    hub.shutdown().await;
    listener.abort();
    let _ = listener.await;
    let _ = std::fs::remove_file(directory.path().join("control.sock"));
    let hub = Arc::new(BridgeHub::new());
    let native: Arc<dyn AgentAdapter> = Arc::new(BridgeAdapter::new(
        AgentKind::OpenCode,
        hub.clone(),
        directory.path().join("bridge"),
    ));
    let agent: Arc<dyn AgentAdapter> =
        Arc::new(AgentRegistry::new(vec![(AgentKind::OpenCode, native)]).unwrap());
    let channel = Arc::new(agentix_telegram::TelegramAdapter::with_bot(
        Bot::new("fixture-token").set_api_url(telegram.api_url().parse().unwrap()),
        agentix_telegram::TelegramPolicy::new([42]),
    ));
    let engine = Arc::new(
        Engine::new(agent.clone(), state, vec![channel.clone()]).with_multiplexer_kind(kind),
    );
    assert_eq!(
        engine
            .restore_bindings_deferred()
            .await
            .unwrap()
            .restored_count(),
        1
    );
    let shutdown = CancellationToken::new();
    let (tx, rx) = mpsc::channel(32);
    let channel_task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { channel.run(tx, shutdown).await }
    });
    let engine_task = tokio::spawn(run_engine_loop(engine.clone(), agent, rx, shutdown.clone()));
    tokio::task::yield_now().await;
    let before_replay = telegram.requests().await.len();
    let (control_tx, _control_rx) = mpsc::channel(16);
    let listener = tokio::spawn({
        let endpoint = endpoint.clone();
        let shutdown = shutdown.clone();
        async move { control::serve_with_bridge(&endpoint, control_tx, shutdown, Some(hub)).await }
    });
    let permission = wait_body(&telegram, "OpenCode permission", before_replay).await;
    let token = button(&permission, "Allow once");
    assert_ne!(
        old_token, token,
        "restarted Engine must replace callback capabilities"
    );
    let before_stale = telegram.requests().await.len();
    telegram.push_updates(vec![callback(103, &old_token)]).await;
    wait_body(&telegram, "cb-103", before_stale).await;
    let rejected = engine
        .handle_inbound(agentix_core::InboundEnvelope::action(
            "verify-stale",
            agentix_core::ConversationRef::new(agentix_core::ChannelKind::Telegram, "42"),
            "42",
            &old_token,
        ))
        .await;
    assert!(
        matches!(rejected, Err(agentix_core::EngineError::InvalidAction)),
        "stale capability must be rejected: {rejected:?}"
    );
    telegram.push_updates(vec![callback(104, &token)]).await;
    wait_final(&telegram, "Closed loop tool answer", before).await;
    let before = telegram.requests().await.len();
    telegram
        .push_updates(vec![message(105, "question-loop")])
        .await;
    let question = wait_body(&telegram, "Choose closed loop features", before).await;
    let before_select = telegram.requests().await.len();
    telegram
        .push_updates(vec![callback(106, &button(&question, "Alpha"))])
        .await;
    let selected = wait_body(&telegram, "✓ Alpha", before_select).await;
    let before_select = telegram.requests().await.len();
    telegram
        .push_updates(vec![callback(107, &button(&selected, "Beta"))])
        .await;
    let selected = wait_body(&telegram, "✓ Beta", before_select).await;
    telegram
        .push_updates(vec![callback(108, &button(&selected, "Submit"))])
        .await;
    wait_final(&telegram, "Closed loop tool answer", before).await;
    let before = telegram.requests().await.len();
    telegram
        .push_updates(vec![message(109, "taskix-loop")])
        .await;
    wait_final(&telegram, "Closed loop tool answer", before).await;
    let provider: Value = serde_json::from_str(
        &std::fs::read_to_string(directory.path().join("provider.json")).unwrap(),
    )
    .unwrap();
    assert!(
        provider["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "taskix")
    );
    assert!(
        provider["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message.to_string().contains("schema_version")),
        "real Taskix CLI tool result: {provider}"
    );
    let requests = std::fs::read_to_string(directory.path().join("provider.jsonl")).unwrap();
    assert!(
        requests
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(
                |request| request["messages"].as_array().unwrap().iter().any(
                    |message| message["role"] == "tool"
                        && message.to_string().contains("Alpha")
                        && message.to_string().contains("Beta")
                )
            ),
        "native question must receive both selected answers"
    );
    assert!(directory.path().join("tasks.sqlite3").exists());
    assert!(telegram.requests().await.iter().any(|r| {
        r.target
            .to_ascii_lowercase()
            .contains("answercallbackquery")
    }));
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), engine_task)
        .await
        .unwrap()
        .unwrap();
    channel_task.abort();
    listener.abort();
}

/// Run the production default rmux endpoint in a fresh process so environment
/// changes never race parallel tests or touch a developer's default server.
#[test]
#[ignore = "requires native OpenCode, rmux and Taskix"]
fn opencode_native_rmux_telegram_closed_loop() {
    let root = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "opencode_closedloop_tests::opencode_native_telegram_engine_permission_and_multiselect_closed_loop",
        "--ignored",
        "--nocapture",
    ]);
    command.env("AGENTIX_TEST_MULTIPLEXER", "rmux");
    command.env("RMUX_TMPDIR", root.path());
    for key in ["RMUX_SDK_ENDPOINT", "TMUX_TMPDIR", "TMUX", "RMUX"] {
        command.env_remove(key);
    }
    let status = command.status().unwrap();
    assert!(status.success());
}
