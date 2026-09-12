//! rmux SDK adapter. Application policy lives in agentix-multiplexer.
use agentix_domain::{MultiplexerKind, MultiplexerTarget, PaneSplitDirection, TerminalLocation};
use agentix_multiplexer::{
    MultiplexerDriver, MultiplexerError, MultiplexerOutcome as RmuxOutcome,
    PaneState as RmuxPaneState, PreparedMutation, command_basename, terminal_location,
};
use async_trait::async_trait;
use rmux_sdk::{
    EnsureSession, Pane, PaneId, PaneProcessState, Rmux, RmuxEndpoint, SessionName, SplitDirection,
};
use std::path::Path;
use std::time::Duration;
use thiserror::Error;
const RMUX_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Debug, Error)]
pub enum RmuxManagerError {
    #[error("rmux SDK request failed: {0}")]
    Sdk(#[from] rmux_sdk::RmuxError),
    #[error("invalid multiplexer target: {0}")]
    InvalidTarget(String),
}
#[derive(Debug, Default)]
pub struct RmuxDriver;
#[async_trait]
impl MultiplexerDriver for RmuxDriver {
    fn kind(&self) -> MultiplexerKind {
        MultiplexerKind::Rmux
    }
    async fn inventory(&self, start: bool) -> Result<Option<Vec<RmuxPaneState>>, MultiplexerError> {
        rmux_inventory(start)
            .await
            .map_err(|e| MultiplexerError::Backend(e.to_string()))
    }
    async fn execute(
        &self,
        prepared: &PreparedMutation,
        argv: Option<&[String]>,
    ) -> Result<RmuxOutcome, MultiplexerError> {
        self.execute_sdk(prepared, argv)
            .await
            .map_err(|e| MultiplexerError::Backend(e.to_string()))
    }
}
impl RmuxDriver {
    /// Verify the native protocol without starting a daemon.
    pub async fn probe() -> Result<bool, RmuxManagerError> {
        probe_endpoint(RmuxEndpoint::Default).await
    }

    async fn execute_sdk(
        &self,
        prepared: &PreparedMutation,
        argv: Option<&[String]>,
    ) -> Result<RmuxOutcome, RmuxManagerError> {
        let rmux = connect_rmux(false).await?.ok_or_else(|| {
            RmuxManagerError::InvalidTarget("rmux stopped before the operation completed".into())
        })?;
        let input_clear_key = input_clear_key_before_launch(&prepared.mutation.target);
        let pane_id = match &prepared.mutation.target {
            MultiplexerTarget::NewSession { name, .. } => {
                let session = rmux
                    .ensure_session(
                        EnsureSession::try_named(name)?
                            .create_only()
                            .detached(true)
                            .working_directory(prepared.cwd.to_string_lossy())
                            .window_name(name),
                    )
                    .await?;
                let pane = session.pane(0, 0);
                launch_in_pane(&pane, argv, &prepared.cwd, input_clear_key).await?;
                required_pane_id(&pane).await?
            }
            MultiplexerTarget::NewWindow {
                session_id, name, ..
            } => {
                let session_name = session_name_for_id(session_id).await?;
                let session = rmux.session(session_name).await?;
                let window = session
                    .new_window_with()
                    .name(name)
                    .cwd(&prepared.cwd)
                    .detached(true)
                    .await?;
                let pane_id = window
                    .panes()
                    .await?
                    .into_iter()
                    .next()
                    .map(|pane| pane.id)
                    .ok_or_else(|| {
                        RmuxManagerError::InvalidTarget(
                            "rmux created a window without a pane".into(),
                        )
                    })?;
                let pane = session.pane_by_id(pane_id).await?;
                launch_in_pane(&pane, argv, &prepared.cwd, input_clear_key).await?;
                pane_id
            }
            MultiplexerTarget::SplitPane {
                pane_id, direction, ..
            } => {
                let pane = pane_by_text_id(&rmux, pane_id).await?;
                let direction = sdk_split_direction(*direction);
                let created = if let Some(argv) = argv {
                    pane.split_with(direction)
                        .spawn(persistent_launch_argv(argv))
                        .cwd(&prepared.cwd)
                        .keep_alive_on_exit(false)
                        .await?
                } else {
                    pane.split(direction).await?
                };
                required_pane_id(&created).await?
            }
            MultiplexerTarget::ExistingPane { pane_id } => {
                let pane = pane_by_text_id(&rmux, pane_id).await?;
                launch_in_pane(&pane, argv, &prepared.cwd, input_clear_key).await?;
                required_pane_id(&pane).await?
            }
        };
        let location = location_for_pane(pane_id).await?;
        Ok(RmuxOutcome { location })
    }
}
async fn probe_endpoint(endpoint: RmuxEndpoint) -> Result<bool, RmuxManagerError> {
    let rmux = Rmux::builder()
        .endpoint(endpoint)
        .default_timeout(RMUX_OPERATION_TIMEOUT)
        .connect()
        .await?;
    rmux.find_sessions().all().await?;
    Ok(true)
}

async fn connect_rmux(start: bool) -> Result<Option<Rmux>, RmuxManagerError> {
    let builder = Rmux::builder()
        .endpoint(RmuxEndpoint::Default)
        .default_timeout(RMUX_OPERATION_TIMEOUT);
    if start {
        return builder
            .connect_or_start()
            .await
            .map(Some)
            .map_err(Into::into);
    }
    match builder.connect().await {
        Ok(rmux) => Ok(Some(rmux)),
        Err(_) => Ok(None),
    }
}

async fn rmux_inventory(start: bool) -> Result<Option<Vec<RmuxPaneState>>, RmuxManagerError> {
    let Some(rmux) = connect_rmux(start).await? else {
        return Ok(None);
    };
    let sessions = rmux.find_sessions().all().await?;
    let mut inventory = Vec::new();
    for discovered_session in sessions {
        let session_name = discovered_session.name.to_string();
        let panes = rmux.find_panes().session(&session_name).all().await?;
        for discovered in panes {
            let window = discovered_session.session.window(discovered.window_index);
            let listed_panes = window.panes().await?;
            let active = listed_panes
                .iter()
                .any(|pane| pane.id == discovered.pane_id && pane.active);
            let info = discovered.pane.info().await?;
            let window_name = info
                .window(discovered.window_id)
                .and_then(|window| window.name.clone())
                .unwrap_or_default();
            let foreground = discovered.pane.foreground_state().await?;
            let current_command = foreground
                .as_ref()
                .and_then(|state| state.command.clone())
                .or_else(|| {
                    discovered
                        .command
                        .as_ref()
                        .and_then(|argv| argv.first().cloned())
                })
                .map(|command| command_basename(&command))
                .unwrap_or_default();
            let cwd = foreground
                .as_ref()
                .and_then(|state| state.cwd.clone())
                .or(discovered.working_directory)
                .unwrap_or_default();
            let foreground_pid = foreground.as_ref().and_then(|state| state.pid).or({
                if let PaneProcessState::Running { pid } = discovered.process {
                    pid
                } else {
                    None
                }
            });
            inventory.push(RmuxPaneState {
                multiplexer: MultiplexerKind::Rmux,
                session_id: discovered.session_id.to_string(),
                session_name: session_name.clone(),
                window_id: discovered.window_id.to_string(),
                window_index: discovered.window_index,
                window_name,
                pane_id: discovered.pane_id.to_string(),
                pane_index: discovered.pane_index,
                active,
                current_command,
                cwd,
                foreground_pid,
            });
        }
    }
    Ok(Some(inventory))
}

async fn session_name_for_id(session_id: &str) -> Result<SessionName, RmuxManagerError> {
    let inventory = rmux_inventory(false)
        .await?
        .ok_or_else(|| RmuxManagerError::InvalidTarget("rmux is not running".into()))?;
    inventory
        .iter()
        .find(|pane| pane.session_id == session_id)
        .map(|pane| {
            SessionName::new(&pane.session_name)
                .map_err(|error| RmuxManagerError::InvalidTarget(error.to_string()))
        })
        .transpose()?
        .ok_or_else(|| {
            RmuxManagerError::InvalidTarget(format!("session {session_id} no longer exists"))
        })
}

async fn pane_by_text_id(rmux: &Rmux, pane_id: &str) -> Result<Pane, RmuxManagerError> {
    let inventory = rmux_inventory(false)
        .await?
        .ok_or_else(|| RmuxManagerError::InvalidTarget("rmux is not running".into()))?;
    let state = inventory
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .ok_or_else(|| {
            RmuxManagerError::InvalidTarget(format!("pane {pane_id} no longer exists"))
        })?;
    Ok(rmux
        .pane_by_id(
            SessionName::new(&state.session_name)
                .map_err(|error| RmuxManagerError::InvalidTarget(error.to_string()))?,
            parse_pane_id(pane_id)?,
        )
        .await?)
}

async fn launch_in_pane(
    pane: &Pane,
    argv: Option<&[String]>,
    cwd: &Path,
    input_clear_key: Option<&str>,
) -> Result<(), RmuxManagerError> {
    let Some(argv) = argv else {
        return Ok(());
    };
    if let Some(key) = input_clear_key {
        pane.send_key(key).await?;
    }
    pane.spawn(persistent_launch_argv(argv))
        .cwd(cwd)
        .kill_existing(true)
        .keep_alive_on_exit(false)
        .await?;
    Ok(())
}

// Keep argv separate from shell syntax, then restore an interactive prompt even
// when the agent exits unsuccessfully. A dead-pane setting alone cannot do this.
#[cfg(unix)]
fn persistent_launch_argv(argv: &[String]) -> Vec<String> {
    let mut command = vec![
        "/bin/sh".to_owned(),
        // Give the agent its own foreground process group for detection and
        // terminal signals while the wrapper waits for it to exit.
        "-i".to_owned(),
        "-c".to_owned(),
        r#""$@"; exec "${SHELL:-/bin/sh}" -i"#.to_owned(),
        "agentix".to_owned(),
    ];
    command.extend_from_slice(argv);
    command
}

#[cfg(not(unix))]
fn persistent_launch_argv(argv: &[String]) -> Vec<String> {
    argv.to_vec()
}

fn input_clear_key_before_launch(target: &MultiplexerTarget) -> Option<&'static str> {
    matches!(target, MultiplexerTarget::ExistingPane { .. }).then_some("C-c")
}

async fn required_pane_id(pane: &Pane) -> Result<PaneId, RmuxManagerError> {
    pane.id().await?.ok_or_else(|| {
        RmuxManagerError::InvalidTarget("rmux pane disappeared after creation".into())
    })
}

async fn location_for_pane(pane_id: PaneId) -> Result<TerminalLocation, RmuxManagerError> {
    let pane_id = pane_id.to_string();
    let inventory = rmux_inventory(false)
        .await?
        .ok_or_else(|| RmuxManagerError::InvalidTarget("rmux is not running".into()))?;
    inventory
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .map(terminal_location)
        .ok_or_else(|| RmuxManagerError::InvalidTarget(format!("pane {pane_id} no longer exists")))
}

fn sdk_split_direction(direction: PaneSplitDirection) -> SplitDirection {
    match direction {
        PaneSplitDirection::Horizontal => SplitDirection::Right,
        PaneSplitDirection::Vertical => SplitDirection::Down,
    }
}

fn parse_pane_id(value: &str) -> Result<PaneId, RmuxManagerError> {
    value
        .strip_prefix('%')
        .and_then(|value| value.parse::<u32>().ok())
        .map(PaneId::new)
        .ok_or_else(|| RmuxManagerError::InvalidTarget(format!("invalid rmux pane id {value}")))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    #[cfg(unix)]
    use rmux_proto::{
        CommandOutput, FrameDecoder, HandshakeResponse, HasSessionResponse, ListPanesResponse,
        NewSessionResponse, NewWindowResponse, PaneId, PaneInputRequest, PaneTarget,
        ProcessCommand, Request, RespawnPaneResponse, Response, SendKeysResponse,
        SplitWindowIdentityResponse, WindowTarget, encode_frame,
    };
    #[cfg(unix)]
    use rmux_sdk::{EnsureSession, Rmux, RmuxEndpoint, SessionName, SplitDirection};
    #[cfg(unix)]
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[cfg(unix)]
    use tokio::net::UnixListener;
    #[cfg(unix)]
    use tokio::sync::Mutex;

    use super::{launch_in_pane, persistent_launch_argv};

    #[cfg(unix)]
    #[cfg(unix)]
    #[tokio::test]
    async fn probe_verifies_native_protocol_and_never_starts_a_server() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("probe.sock");
        assert!(
            super::probe_endpoint(RmuxEndpoint::UnixSocket(socket.clone()))
                .await
                .is_err()
        );
        assert!(!socket.exists());
        let listener = UnixListener::bind(&socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server = spawn_mock_rmux(listener, requests.clone());
        assert!(
            super::probe_endpoint(RmuxEndpoint::UnixSocket(socket))
                .await
                .unwrap()
        );
        assert!(!requests.lock().await.is_empty());
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_rejects_a_socket_that_does_not_speak_rmux() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("foreign.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"not the rmux protocol\n").await.unwrap();
        });
        assert!(
            super::probe_endpoint(RmuxEndpoint::UnixSocket(socket))
                .await
                .is_err()
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn reused_pane_clear_and_codex_launch_cross_the_rmux_sdk_wire() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("rmux.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server = spawn_mock_rmux(listener, requests.clone());
        let rmux = Rmux::builder()
            .endpoint(RmuxEndpoint::UnixSocket(socket))
            .connect()
            .await
            .unwrap();
        let session = rmux
            .session(SessionName::new("agentix").unwrap())
            .await
            .unwrap();
        let pane = session.pane_by_id(PaneId::new(1)).await.unwrap();
        let argv = vec![
            "/opt/codex".to_owned(),
            "--remote".to_owned(),
            "unix:///tmp/codex.sock".to_owned(),
        ];

        launch_in_pane(&pane, Some(&argv), Path::new("/work/agentix"), Some("C-c"))
            .await
            .unwrap();
        drop(pane);
        drop(session);
        drop(rmux);
        server.abort();

        let requests = requests.lock().await;
        assert!(requests.iter().any(|request| matches!(
            request,
            Request::PaneInput(PaneInputRequest { keys, literal, .. })
                if keys == &["C-c"] && !literal
        )));
        let respawn = requests
            .iter()
            .find_map(|request| match request {
                Request::PaneRespawn(request) => Some(request),
                _ => None,
            })
            .unwrap();
        assert!(respawn.kill);
        assert_eq!(respawn.keep_alive_on_exit, Some(false));
        assert_eq!(
            respawn.start_directory.as_deref(),
            Some(Path::new("/work/agentix"))
        );
        let Some(ProcessCommand::Argv(command)) = &respawn.process_command else {
            panic!("expected structured argv");
        };
        assert_returns_to_shell(command, &argv);
    }

    #[cfg(unix)]
    fn assert_returns_to_shell(command: &[String], agent_argv: &[String]) {
        use std::io::Write;
        use std::process::{Command, Stdio};

        // Replace only the agent argv with a deterministic child, retaining the
        // actual SDK launch wrapper. Exercise success, failure and literal args.
        assert!(command.ends_with(agent_argv));
        for (status, shell) in [(0, Some("/bin/sh")), (7, None), (0, Some(""))] {
            let mut launch = command[..command.len() - agent_argv.len()].to_vec();
            launch.extend([
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!("printf '%s\\n' \"$1\"; exit {status}"),
                "test-agent".to_owned(),
                "literal ' \" $HOME ; $(exit 99)".to_owned(),
            ]);
            let mut process = Command::new(&launch[0]);
            if let Some(shell) = shell {
                process.env("SHELL", shell);
            } else {
                process.env_remove("SHELL");
            }
            let mut child = process
                .args(&launch[1..])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"echo pane-still-usable\nexit\n")
                .unwrap();
            let output = child.wait_with_output().unwrap();
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(
                stdout.contains("literal ' \" $HOME ; $(exit 99)"),
                "{stdout}"
            );
            assert!(
                stdout.contains("pane-still-usable"),
                "agent exit {status} left no usable shell: {stdout}"
            );
            assert!(output.status.success());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_window_and_split_creation_cross_the_rmux_sdk_wire() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("rmux.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server = spawn_mock_rmux(listener, requests.clone());
        let rmux = Rmux::builder()
            .endpoint(RmuxEndpoint::UnixSocket(socket))
            .connect()
            .await
            .unwrap();
        let session = rmux
            .ensure_session(
                EnsureSession::try_named("created")
                    .unwrap()
                    .create_only()
                    .detached(true)
                    .working_directory("/work/created")
                    .window_name("created"),
            )
            .await
            .unwrap();
        let window = session
            .new_window_with()
            .name("codex")
            .cwd("/work/window")
            .detached(true)
            .await
            .unwrap();
        let pane = session.pane_by_id(PaneId::new(1)).await.unwrap();
        let argv = vec!["/opt/codex".to_owned(), "--remote".to_owned()];
        let split = pane
            .split_with(SplitDirection::Right)
            .spawn(persistent_launch_argv(&argv))
            .cwd("/work/split")
            .keep_alive_on_exit(false)
            .await
            .unwrap();
        drop(split);
        drop(pane);
        drop(window);
        drop(session);
        drop(rmux);
        server.abort();

        let requests = requests.lock().await;
        let split_command = requests
            .iter()
            .find_map(|request| match request {
                Request::SplitWindowIdentity(request) => request.action.process_command.as_ref(),
                _ => None,
            })
            .unwrap();
        let ProcessCommand::Argv(command) = split_command else {
            panic!("expected structured argv");
        };
        assert_returns_to_shell(command, &argv);
        assert!(requests.iter().any(|request| matches!(
            request,
            Request::NewSessionExt(request)
                if request.session_name.as_ref().is_some_and(|name| name.as_str() == "created")
                    && request.working_directory.as_deref() == Some("/work/created")
                    && request.window_name.as_deref() == Some("created")
                    && request.detached
        )));
        assert!(requests.iter().any(|request| matches!(
            request,
            Request::NewWindow(request)
                if request.name.as_deref() == Some("codex")
                    && request.start_directory.as_deref() == Some(Path::new("/work/window"))
                    && request.detached
        )));
        assert!(requests.iter().any(|request| matches!(
            request,
            Request::SplitWindowIdentity(request)
                if request.action.start_directory.as_deref() == Some(Path::new("/work/split"))
                    && request.action.keep_alive_on_exit == Some(false)
                    && request.action.process_command == Some(ProcessCommand::Argv(persistent_launch_argv(&argv)))
        )));
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires an installed rmux daemon"]
    async fn live_rmux_agent_exit_restores_usable_pane() {
        use std::process::Command;
        use std::time::Duration;

        struct Server(std::path::PathBuf);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = Command::new("rmux")
                    .arg("-S")
                    .arg(&self.0)
                    .arg("kill-server")
                    .output();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let server = Server(directory.path().join("rmux.sock"));
        let output = Command::new("rmux")
            .arg("-S")
            .arg(&server.0)
            .args(["-f", "/dev/null", "new-session", "-d", "-s", "pane-test"])
            .env("SHELL", "/bin/sh")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let rmux = Rmux::builder()
            .endpoint(RmuxEndpoint::UnixSocket(server.0.clone()))
            .connect()
            .await
            .unwrap();
        let session = rmux
            .session(SessionName::new("pane-test").unwrap())
            .await
            .unwrap();
        let window = session
            .new_window_with()
            .name("agent")
            .cwd(directory.path())
            .detached(true)
            .await
            .unwrap();
        let pane_id = window.panes().await.unwrap()[0].id;
        let pane = session.pane_by_id(pane_id).await.unwrap();
        launch_in_pane(
            &pane,
            Some(&["/bin/sleep".into(), "30".into()]),
            directory.path(),
            None,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = pane.foreground_state().await.unwrap();

                if state
                    .as_ref()
                    .and_then(|s| s.command.as_deref())
                    .is_some_and(|c| c.ends_with("sleep"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("running agent must remain visible as the foreground command");
        pane.send_key("C-c").await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = pane.foreground_state().await.unwrap();

                if state
                    .as_ref()
                    .and_then(|s| s.command.as_deref())
                    .is_some_and(|c| c.ends_with("sh"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("agent interruption must restore a shell");
        pane.send_text("echo usable > pane-result").await.unwrap();
        pane.send_key("Enter").await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !directory.path().join("pane-result").exists() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("restored shell must accept commands in the launch directory");
        assert_eq!(
            std::fs::read_to_string(directory.path().join("pane-result")).unwrap(),
            "usable\n"
        );
        assert_ctrl_d_closes_pane(&rmux, &pane, pane_id).await;
    }

    #[cfg(unix)]
    async fn assert_ctrl_d_closes_pane(rmux: &Rmux, pane: &rmux_sdk::Pane, id: PaneId) {
        pane.send_key("C-d").await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match rmux.find_panes().all().await {
                    Ok(panes) => {
                        assert!(!panes.is_empty(), "the other window must stay open");
                        if panes.iter().all(|pane| pane.pane_id != id) {
                            break;
                        }
                    }
                    // Discovery can race the pane disappearing between SDK requests.
                    Err(rmux_sdk::RmuxError::PaneNotFound { pane_id, .. }) if pane_id == id => {}
                    Err(error) => panic!("unexpected inventory failure: {error}"),
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Ctrl-D must remove the pane, not leave a dead pane");
    }

    #[cfg(unix)]
    fn spawn_mock_rmux(
        listener: UnixListener,
        requests: Arc<Mutex<Vec<Request>>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let requests = requests.clone();
                tokio::spawn(async move {
                    let mut decoder = FrameDecoder::new();
                    let mut buffer = [0_u8; 4_096];
                    loop {
                        while let Some(request) = decoder.next_frame::<Request>().unwrap() {
                            requests.lock().await.push(request.clone());
                            let response = mock_rmux_response(request);
                            stream
                                .write_all(&encode_frame(&response).unwrap())
                                .await
                                .unwrap();
                        }
                        let read = stream.read(&mut buffer).await.unwrap();
                        if read == 0 {
                            break;
                        }
                        decoder.push_bytes(&buffer[..read]);
                    }
                });
            }
        })
    }

    #[cfg(unix)]
    fn mock_rmux_response(request: Request) -> Response {
        match request {
            Request::Handshake(_) => Response::Handshake(HandshakeResponse::current()),
            Request::ListSessions(_) => Response::ListSessions(rmux_proto::ListSessionsResponse {
                output: CommandOutput::from_stdout(""),
            }),
            Request::HasSession(_) => Response::HasSession(HasSessionResponse { exists: true }),
            Request::ListPanes(_) => Response::ListPanes(ListPanesResponse {
                output: CommandOutput::from_stdout("0:0:%1\n1:0:%2\n"),
            }),
            Request::NewSessionExt(request) => Response::NewSession(NewSessionResponse {
                session_name: request
                    .session_name
                    .unwrap_or_else(|| SessionName::new("created").unwrap()),
                detached: request.detached,
                output: None,
            }),
            Request::NewWindow(request) => Response::NewWindow(NewWindowResponse {
                target: WindowTarget::with_window(request.target.clone(), 1),
            }),
            Request::PaneInput(request) => Response::SendKeys(SendKeysResponse {
                key_count: request.keys.len(),
            }),
            Request::PaneRespawn(_) => Response::RespawnPane(RespawnPaneResponse {
                target: PaneTarget::new(SessionName::new("agentix").unwrap(), 0),
            }),
            Request::SplitWindowIdentity(request) => {
                let _ = request;
                Response::SplitWindowIdentity(SplitWindowIdentityResponse {
                    pane: PaneTarget::new(SessionName::new("created").unwrap(), 1),
                    pane_id: PaneId::new(3),
                })
            }
            request => panic!("unexpected mock rmux request: {request:?}"),
        }
    }
}
