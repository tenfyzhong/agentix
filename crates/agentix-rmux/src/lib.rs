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
                        .spawn(argv.iter().cloned())
                        .cwd(&prepared.cwd)
                        .keep_alive_on_exit(true)
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
    pane.spawn(argv.iter().cloned())
        .cwd(cwd)
        .kill_existing(true)
        .keep_alive_on_exit(true)
        .await?;
    Ok(())
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

    use super::launch_in_pane;

    #[cfg(unix)]
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
        assert!(requests.iter().any(|request| matches!(
            request,
            Request::PaneRespawn(request)
                if request.kill
                    && request.keep_alive_on_exit == Some(true)
                    && request.start_directory.as_deref() == Some(Path::new("/work/agentix"))
                    && request.process_command == Some(ProcessCommand::Argv(argv.clone()))
        )));
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
            .spawn(argv.clone())
            .cwd("/work/split")
            .keep_alive_on_exit(true)
            .await
            .unwrap();
        drop(split);
        drop(pane);
        drop(window);
        drop(session);
        drop(rmux);
        server.abort();

        let requests = requests.lock().await;
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
                    && request.action.keep_alive_on_exit == Some(true)
                    && request.action.process_command == Some(ProcessCommand::Argv(argv.clone()))
        )));
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
