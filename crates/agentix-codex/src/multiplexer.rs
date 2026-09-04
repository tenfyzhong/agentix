use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentix_core::{
    MultiplexerMutation, MultiplexerPane, MultiplexerSession, MultiplexerSnapshot,
    MultiplexerTarget, MultiplexerWindow, PaneSplitDirection, SessionId, SessionSummary,
    TerminalLocation,
};
use rmux_sdk::{
    EnsureSession, Pane, PaneId, PaneProcessState, Rmux, RmuxEndpoint, SessionName, SplitDirection,
};
use thiserror::Error;

const RMUX_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub(crate) enum RmuxManagerError {
    #[error("rmux SDK request failed: {0}")]
    Sdk(#[from] rmux_sdk::RmuxError),
    #[error("invalid multiplexer target: {0}")]
    InvalidTarget(String),
    #[error("invalid multiplexer name: use 1-64 ASCII letters, numbers, '.', '_' or '-'")]
    InvalidName,
    #[error("workspace is unavailable: {0}")]
    InvalidWorkspace(String),
    #[error("pane {pane_id} is busy running {command}")]
    BusyPane { pane_id: String, command: String },
}

#[derive(Debug, Clone)]
pub(crate) struct RmuxManager {
    codex_command: PathBuf,
    remote_address: String,
    default_directory: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedMutation {
    pub(crate) mutation: MultiplexerMutation,
    pub(crate) cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct RmuxOutcome {
    pub(crate) location: TerminalLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RmuxPaneState {
    session_id: String,
    session_name: String,
    window_id: String,
    window_index: u32,
    window_name: String,
    pane_id: String,
    pane_index: u32,
    active: bool,
    current_command: String,
    cwd: String,
    foreground_pid: Option<u32>,
}

impl RmuxManager {
    pub(crate) fn new(codex_command: &Path, socket_path: &Path, default_directory: &Path) -> Self {
        Self {
            codex_command: codex_command.to_owned(),
            remote_address: format!("unix://{}", socket_path.display()),
            default_directory: default_directory.to_owned(),
        }
    }

    pub(crate) fn default_directory(&self) -> &Path {
        &self.default_directory
    }

    pub(crate) async fn snapshot(
        codex_sessions: &[SessionSummary],
    ) -> Result<Option<MultiplexerSnapshot>, RmuxManagerError> {
        let Some(inventory) = rmux_inventory(true).await? else {
            return Ok(None);
        };
        let codex_by_pane = codex_sessions_by_pane(codex_sessions);
        Ok(Some(snapshot_from_inventory(&inventory, &codex_by_pane)))
    }

    pub(crate) async fn prepare(
        mut mutation: MultiplexerMutation,
    ) -> Result<PreparedMutation, RmuxManagerError> {
        let snapshot = Self::snapshot(&[])
            .await?
            .ok_or_else(|| RmuxManagerError::InvalidTarget("rmux is not running".into()))?;
        let cwd = match &mutation.target {
            MultiplexerTarget::NewSession { name, cwd } => {
                validate_name(name)?;
                resolve_workspace(cwd)?
            }
            MultiplexerTarget::NewWindow {
                session_id,
                name,
                cwd,
            } => {
                validate_name(name)?;
                if !snapshot
                    .sessions
                    .iter()
                    .any(|session| session.id == *session_id)
                {
                    return Err(RmuxManagerError::InvalidTarget(format!(
                        "session {session_id} no longer exists"
                    )));
                }
                resolve_workspace(cwd)?
            }
            MultiplexerTarget::SplitPane { pane_id, cwd, .. } => {
                find_pane(&snapshot, pane_id).ok_or_else(|| {
                    RmuxManagerError::InvalidTarget(format!("pane {pane_id} no longer exists"))
                })?;
                resolve_workspace(cwd)?
            }
            MultiplexerTarget::ExistingPane { pane_id } => {
                let pane = find_pane(&snapshot, pane_id).ok_or_else(|| {
                    RmuxManagerError::InvalidTarget(format!("pane {pane_id} no longer exists"))
                })?;
                if !is_shell_command(&pane.current_command) {
                    return Err(RmuxManagerError::BusyPane {
                        pane_id: pane.id.clone(),
                        command: pane.current_command.clone(),
                    });
                }
                resolve_workspace(&pane.cwd)?
            }
        };
        if let MultiplexerTarget::NewSession { name, .. } = &mut mutation.target {
            *name = available_session_name(&snapshot, name);
        }
        if !mutation.launch_codex
            && matches!(mutation.target, MultiplexerTarget::ExistingPane { .. })
        {
            return Err(RmuxManagerError::InvalidTarget(
                "an existing pane can only launch Codex".into(),
            ));
        }
        Ok(PreparedMutation { mutation, cwd })
    }

    pub(crate) async fn execute(
        &self,
        prepared: &PreparedMutation,
    ) -> Result<RmuxOutcome, RmuxManagerError> {
        let rmux = connect_rmux(false).await?.ok_or_else(|| {
            RmuxManagerError::InvalidTarget("rmux stopped before the operation completed".into())
        })?;
        let argv = prepared
            .mutation
            .launch_codex
            .then(|| build_codex_argv(&self.codex_command, &self.remote_address, &prepared.cwd));
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
                launch_in_pane(&pane, argv.as_deref(), &prepared.cwd, input_clear_key).await?;
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
                launch_in_pane(&pane, argv.as_deref(), &prepared.cwd, input_clear_key).await?;
                pane_id
            }
            MultiplexerTarget::SplitPane {
                pane_id, direction, ..
            } => {
                let pane = pane_by_text_id(&rmux, pane_id).await?;
                let direction = sdk_split_direction(*direction);
                let created = if let Some(argv) = argv.as_deref() {
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
                launch_in_pane(&pane, argv.as_deref(), &prepared.cwd, input_clear_key).await?;
                required_pane_id(&pane).await?
            }
        };
        let location = location_for_pane(pane_id).await?;
        Ok(RmuxOutcome { location })
    }

    pub(crate) async fn pane_exists(location: &TerminalLocation) -> Result<bool, RmuxManagerError> {
        let Some(inventory) = rmux_inventory(false).await? else {
            return Ok(false);
        };
        Ok(inventory
            .iter()
            .any(|pane| pane.pane_id == location.pane_id))
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

pub(crate) async fn rmux_process_locations()
-> Result<HashMap<u32, TerminalLocation>, RmuxManagerError> {
    let Some(inventory) = rmux_inventory(false).await? else {
        return Ok(HashMap::new());
    };
    Ok(inventory
        .into_iter()
        .filter_map(|pane| {
            pane.foreground_pid
                .map(|pid| (pid, terminal_location(&pane)))
        })
        .collect())
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

fn terminal_location(pane: &RmuxPaneState) -> TerminalLocation {
    TerminalLocation {
        session: pane.session_name.clone(),
        window_index: pane.window_index.to_string(),
        window_name: pane.window_name.clone(),
        pane_index: pane.pane_index.to_string(),
        pane_id: pane.pane_id.clone(),
    }
}

fn snapshot_from_inventory(
    inventory: &[RmuxPaneState],
    codex_sessions: &HashMap<String, SessionId>,
) -> MultiplexerSnapshot {
    let mut sessions = Vec::<MultiplexerSession>::new();
    for pane in inventory {
        let session_position = sessions
            .iter()
            .position(|session| session.id == pane.session_id)
            .unwrap_or_else(|| {
                sessions.push(MultiplexerSession {
                    id: pane.session_id.clone(),
                    name: pane.session_name.clone(),
                    windows: Vec::new(),
                });
                sessions.len() - 1
            });
        let session = &mut sessions[session_position];
        let window_position = session
            .windows
            .iter()
            .position(|window| window.id == pane.window_id)
            .unwrap_or_else(|| {
                session.windows.push(MultiplexerWindow {
                    id: pane.window_id.clone(),
                    index: pane.window_index.to_string(),
                    name: pane.window_name.clone(),
                    panes: Vec::new(),
                });
                session.windows.len() - 1
            });
        session.windows[window_position]
            .panes
            .push(MultiplexerPane {
                id: pane.pane_id.clone(),
                index: pane.pane_index.to_string(),
                active: pane.active,
                current_command: pane.current_command.clone(),
                cwd: pane.cwd.clone(),
                codex_session: codex_sessions.get(&pane.pane_id).cloned(),
            });
    }
    sessions.sort_by(|left, right| left.name.cmp(&right.name));
    for session in &mut sessions {
        session
            .windows
            .sort_by_key(|window| window.index.parse::<u32>().unwrap_or(u32::MAX));
        for window in &mut session.windows {
            window
                .panes
                .sort_by_key(|pane| pane.index.parse::<u32>().unwrap_or(u32::MAX));
        }
    }
    MultiplexerSnapshot { sessions }
}

pub(crate) fn session_at_location<'a>(
    sessions: &'a [SessionSummary],
    location: &TerminalLocation,
) -> Option<&'a SessionSummary> {
    sessions.iter().find(|session| {
        session
            .terminal
            .as_ref()
            .is_some_and(|terminal| terminal.pane_id == location.pane_id)
    })
}

pub(crate) fn started_session<'a>(
    sessions: &'a [SessionSummary],
    location: &TerminalLocation,
    known_sessions: &HashSet<SessionId>,
    cwd: &Path,
) -> Option<&'a SessionSummary> {
    if let Some(session) = session_at_location(sessions, location)
        .filter(|session| !known_sessions.contains(&session.id))
    {
        return Some(session);
    }
    let mut candidates = sessions.iter().filter(|session| {
        !known_sessions.contains(&session.id)
            && session
                .cwd
                .as_deref()
                .is_some_and(|value| Path::new(value) == cwd)
    });
    let candidate = candidates.next()?;
    candidates.next().is_none().then_some(candidate)
}

fn codex_sessions_by_pane(sessions: &[SessionSummary]) -> HashMap<String, SessionId> {
    sessions
        .iter()
        .filter_map(|session| {
            session
                .terminal
                .as_ref()
                .map(|terminal| (terminal.pane_id.clone(), session.id.clone()))
        })
        .collect()
}

fn find_pane<'a>(snapshot: &'a MultiplexerSnapshot, pane_id: &str) -> Option<&'a MultiplexerPane> {
    snapshot
        .sessions
        .iter()
        .flat_map(|session| &session.windows)
        .flat_map(|window| &window.panes)
        .find(|pane| pane.id == pane_id)
}

fn available_session_name(snapshot: &MultiplexerSnapshot, requested: &str) -> String {
    if !snapshot
        .sessions
        .iter()
        .any(|session| session.name == requested)
    {
        return requested.into();
    }
    (2..=snapshot.sessions.len() + 2)
        .map(|suffix| format!("{requested}-{suffix}"))
        .find(|candidate| {
            !snapshot
                .sessions
                .iter()
                .any(|session| session.name == *candidate)
        })
        .expect("an available session suffix must exist")
}

fn validate_name(name: &str) -> Result<(), RmuxManagerError> {
    if !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        Ok(())
    } else {
        Err(RmuxManagerError::InvalidName)
    }
}

fn resolve_workspace(value: &str) -> Result<PathBuf, RmuxManagerError> {
    let path = if value == "~" {
        dirs::home_dir().ok_or_else(|| RmuxManagerError::InvalidWorkspace(value.into()))?
    } else if let Some(suffix) = value.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or_else(|| RmuxManagerError::InvalidWorkspace(value.into()))?
            .join(suffix)
    } else {
        PathBuf::from(value)
    };
    std::fs::canonicalize(&path)
        .map_err(|_| RmuxManagerError::InvalidWorkspace(path.display().to_string()))
}

fn is_shell_command(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            matches!(
                name,
                "bash" | "dash" | "elvish" | "fish" | "ksh" | "nu" | "sh" | "tcsh" | "zsh"
            )
        })
}

fn command_basename(command: &str) -> String {
    Path::new(command)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(command)
        .to_owned()
}

fn build_codex_argv(command: &Path, remote_address: &str, cwd: &Path) -> Vec<String> {
    vec![
        command.to_string_lossy().into_owned(),
        "--remote".into(),
        remote_address.into(),
        "-C".into(),
        cwd.to_string_lossy().into_owned(),
    ]
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
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    use std::sync::Arc;

    use agentix_core::{
        MultiplexerSession, MultiplexerSnapshot, MultiplexerTarget, MultiplexerWindow,
        PaneSplitDirection, SessionId, SessionStatus, SessionSummary, TerminalLocation,
    };
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

    use super::{
        RmuxPaneState, available_session_name, build_codex_argv, input_clear_key_before_launch,
        launch_in_pane, session_at_location, snapshot_from_inventory, started_session,
    };

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

    fn snapshot() -> MultiplexerSnapshot {
        MultiplexerSnapshot {
            sessions: vec![MultiplexerSession {
                id: "$1".into(),
                name: "agentix".into(),
                windows: vec![MultiplexerWindow {
                    id: "@1".into(),
                    index: "0".into(),
                    name: "codex".into(),
                    panes: Vec::new(),
                }],
            }],
        }
    }

    #[test]
    fn default_session_name_gets_the_first_available_suffix() {
        let mut existing = snapshot();
        existing.sessions.push(MultiplexerSession {
            id: "$2".into(),
            name: "codex".into(),
            windows: Vec::new(),
        });
        existing.sessions.push(MultiplexerSession {
            id: "$3".into(),
            name: "codex-2".into(),
            windows: Vec::new(),
        });

        assert_eq!(available_session_name(&existing, "codex"), "codex-3");
    }

    #[test]
    fn converts_rmux_sdk_inventory_into_the_ui_hierarchy() {
        let base = RmuxPaneState {
            session_id: "$1".into(),
            session_name: "agentix".into(),
            window_id: "@1".into(),
            window_index: 0,
            window_name: "codex".into(),
            pane_id: "%1".into(),
            pane_index: 0,
            active: true,
            current_command: "codex".into(),
            cwd: "/work/agentix".into(),
            foreground_pid: Some(42),
        };
        let inventory = vec![
            base.clone(),
            RmuxPaneState {
                pane_id: "%2".into(),
                pane_index: 1,
                active: false,
                current_command: "fish".into(),
                foreground_pid: Some(43),
                ..base
            },
        ];
        let codex_sessions = HashMap::from([("%1".into(), SessionId::new("thr_agentix"))]);

        let snapshot = snapshot_from_inventory(&inventory, &codex_sessions);

        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].windows[0].panes.len(), 2);
        assert!(snapshot.sessions[0].windows[0].panes[0].active);
        assert_eq!(
            snapshot.sessions[0].windows[0].panes[0].codex_session,
            Some(SessionId::new("thr_agentix"))
        );
    }

    #[test]
    fn codex_launch_uses_structured_argv() {
        let command = build_codex_argv(
            Path::new("/Applications/Codex CLI/codex"),
            "unix:///tmp/codex socket.sock",
            Path::new("/Users/Test Work/'quoted"),
        );

        assert_eq!(
            command,
            [
                "/Applications/Codex CLI/codex",
                "--remote",
                "unix:///tmp/codex socket.sock",
                "-C",
                "/Users/Test Work/'quoted",
            ]
        );
        assert!(!command.iter().any(|argument| argument == "resume"));
    }

    #[test]
    fn only_reused_panes_clear_unsubmitted_shell_input_before_launch() {
        assert_eq!(
            input_clear_key_before_launch(&MultiplexerTarget::ExistingPane {
                pane_id: "%1".into(),
            }),
            Some("C-c")
        );
        assert_eq!(
            input_clear_key_before_launch(&MultiplexerTarget::NewSession {
                name: "codex".into(),
                cwd: "/work".into(),
            }),
            None
        );
        assert_eq!(
            input_clear_key_before_launch(&MultiplexerTarget::NewWindow {
                session_id: "$1".into(),
                name: "codex".into(),
                cwd: "/work".into(),
            }),
            None
        );
        assert_eq!(
            input_clear_key_before_launch(&MultiplexerTarget::SplitPane {
                pane_id: "%1".into(),
                direction: PaneSplitDirection::Horizontal,
                cwd: "/work".into(),
            }),
            None
        );
    }

    #[test]
    fn finds_only_the_session_running_in_the_created_pane() {
        let target = TerminalLocation {
            session: "agentix".into(),
            window_index: "2".into(),
            window_name: "codex".into(),
            pane_index: "0".into(),
            pane_id: "%9".into(),
        };
        let sessions = [SessionSummary {
            id: SessionId::new("thr_target"),
            name: None,
            preview: None,
            cwd: None,
            updated_at: None,
            status: SessionStatus::Active,
            terminal: Some(target.clone()),
        }];

        assert_eq!(
            session_at_location(&sessions, &target).map(|session| session.id.as_str()),
            Some("thr_target")
        );
    }

    #[test]
    fn finds_the_only_new_session_when_terminal_discovery_is_still_stale() {
        let target = TerminalLocation {
            session: "agentix".into(),
            window_index: "2".into(),
            window_name: "codex".into(),
            pane_index: "0".into(),
            pane_id: "%9".into(),
        };
        let sessions = [
            SessionSummary {
                id: SessionId::new("thr_existing"),
                name: None,
                preview: None,
                cwd: Some("/work/agentix".into()),
                updated_at: None,
                status: SessionStatus::Active,
                terminal: Some(target.clone()),
            },
            SessionSummary {
                id: SessionId::new("thr_started"),
                name: None,
                preview: None,
                cwd: Some("/work/agentix".into()),
                updated_at: None,
                status: SessionStatus::Active,
                terminal: None,
            },
        ];
        let known = HashSet::from([SessionId::new("thr_existing")]);

        assert_eq!(
            started_session(&sessions, &target, &known, Path::new("/work/agentix"))
                .map(|session| session.id.as_str()),
            Some("thr_started")
        );
    }
}
