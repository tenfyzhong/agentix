//! Backend-independent terminal inventory, validation and workspace management.
use agentix_domain::{
    MultiplexerKind, MultiplexerMutation, MultiplexerPane, MultiplexerSession, MultiplexerSnapshot,
    MultiplexerTarget, MultiplexerWindow, SessionId, SessionSummary, TerminalLocation,
};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MultiplexerError {
    #[error("terminal operation failed: {0}")]
    Backend(String),
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
pub struct PreparedMutation {
    pub mutation: MultiplexerMutation,
    pub cwd: PathBuf,
}
#[derive(Debug, Clone)]
pub struct MultiplexerOutcome {
    pub location: TerminalLocation,
}

#[async_trait]
pub trait MultiplexerDriver: std::fmt::Debug + Send + Sync {
    fn kind(&self) -> MultiplexerKind;
    async fn inventory(&self, start: bool) -> Result<Option<Vec<PaneState>>, MultiplexerError>;
    async fn execute(
        &self,
        prepared: &PreparedMutation,
        argv: Option<&[String]>,
    ) -> Result<MultiplexerOutcome, MultiplexerError>;
    async fn process_locations(&self) -> Result<HashMap<u32, TerminalLocation>, MultiplexerError> {
        Ok(self
            .inventory(false)
            .await?
            .unwrap_or_default()
            .into_iter()
            .filter_map(|pane| {
                pane.foreground_pid
                    .map(|pid| (pid, terminal_location(&pane)))
            })
            .collect())
    }
}

/// The configured driver is shared with background discovery clones.
#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    driver: Arc<RwLock<Option<Arc<dyn MultiplexerDriver>>>>,
    argv: Vec<String>,
    default_directory: PathBuf,
}

impl WorkspaceManager {
    #[must_use]
    pub fn new(argv: Vec<String>, default_directory: &Path) -> Self {
        Self {
            driver: Arc::default(),
            argv,
            default_directory: default_directory.into(),
        }
    }
    pub fn set_argv(&mut self, argv: Vec<String>) {
        self.argv = argv;
    }
    pub fn set_driver(&self, driver: Arc<dyn MultiplexerDriver>) {
        *self.driver.write().expect("multiplexer driver lock") = Some(driver);
    }
    fn driver(&self) -> Result<Arc<dyn MultiplexerDriver>, MultiplexerError> {
        self.driver
            .read()
            .expect("multiplexer driver lock")
            .clone()
            .ok_or_else(|| MultiplexerError::Backend("multiplexer is not configured".into()))
    }
    #[must_use]
    pub fn kind(&self) -> MultiplexerKind {
        self.driver
            .read()
            .expect("multiplexer driver lock")
            .as_ref()
            .map_or(MultiplexerKind::default(), |d| d.kind())
    }
    #[must_use]
    pub fn default_directory(&self) -> &Path {
        &self.default_directory
    }
    #[must_use]
    pub fn launch_argv(&self) -> &[String] {
        &self.argv
    }
    pub async fn snapshot(
        &self,
        sessions: &[SessionSummary],
    ) -> Result<Option<MultiplexerSnapshot>, MultiplexerError> {
        let driver = self.driver()?;
        Ok(driver
            .inventory(true)
            .await?
            .map(|panes| snapshot_from_inventory(&panes, &sessions_by_pane(sessions))))
    }
    pub async fn process_locations(
        &self,
    ) -> Result<HashMap<u32, TerminalLocation>, MultiplexerError> {
        let Ok(driver) = self.driver() else {
            return Ok(HashMap::new());
        };
        driver.process_locations().await
    }
    pub async fn pane_exists(&self, location: &TerminalLocation) -> Result<bool, MultiplexerError> {
        let driver = self.driver()?;
        if driver.kind() != location.multiplexer {
            return Ok(false);
        }
        Ok(driver
            .inventory(false)
            .await?
            .unwrap_or_default()
            .iter()
            .any(|p| p.pane_id == location.pane_id))
    }
    pub async fn execute(
        &self,
        prepared: &PreparedMutation,
    ) -> Result<MultiplexerOutcome, MultiplexerError> {
        let driver = self.driver()?;
        let prepared = self.prepare(prepared.mutation.clone()).await?;
        let argv = prepared
            .mutation
            .launch_agent
            .then_some(self.argv.as_slice());
        driver.execute(&prepared, argv).await
    }
    pub async fn prepare(
        &self,
        mut mutation: MultiplexerMutation,
    ) -> Result<PreparedMutation, MultiplexerError> {
        let snapshot = self.snapshot(&[]).await?.ok_or_else(|| {
            MultiplexerError::InvalidTarget("multiplexer is not available".into())
        })?;
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
                    return Err(MultiplexerError::InvalidTarget(format!(
                        "session {session_id} no longer exists"
                    )));
                }
                resolve_workspace(cwd)?
            }
            MultiplexerTarget::SplitPane { pane_id, cwd, .. } => {
                find_pane(&snapshot, pane_id).ok_or_else(|| {
                    MultiplexerError::InvalidTarget(format!("pane {pane_id} no longer exists"))
                })?;
                resolve_workspace(cwd)?
            }
            MultiplexerTarget::ExistingPane { pane_id } => {
                let pane = find_pane(&snapshot, pane_id).ok_or_else(|| {
                    MultiplexerError::InvalidTarget(format!("pane {pane_id} no longer exists"))
                })?;
                if !is_shell_command(&pane.current_command) {
                    return Err(MultiplexerError::BusyPane {
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
        if !mutation.launch_agent
            && matches!(mutation.target, MultiplexerTarget::ExistingPane { .. })
        {
            return Err(MultiplexerError::InvalidTarget(
                "an existing pane must launch an agent".into(),
            ));
        }
        Ok(PreparedMutation { mutation, cwd })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneState {
    pub multiplexer: MultiplexerKind,
    pub session_id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_index: u32,
    pub window_name: String,
    pub pane_id: String,
    pub pane_index: u32,
    pub active: bool,
    pub current_command: String,
    pub cwd: String,
    pub foreground_pid: Option<u32>,
}

#[must_use]
pub fn terminal_location(pane: &PaneState) -> TerminalLocation {
    TerminalLocation {
        multiplexer: pane.multiplexer,
        session: pane.session_name.clone(),
        window_index: pane.window_index.to_string(),
        window_name: pane.window_name.clone(),
        pane_index: pane.pane_index.to_string(),
        pane_id: pane.pane_id.clone(),
    }
}

fn snapshot_from_inventory(
    inventory: &[PaneState],
    sessions_by_terminal: &HashMap<(agentix_domain::MultiplexerKind, String), SessionId>,
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
                agent_session: sessions_by_terminal
                    .get(&(pane.multiplexer, pane.pane_id.clone()))
                    .cloned(),
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

#[must_use]
pub fn session_at_location<'a>(
    sessions: &'a [SessionSummary],
    location: &TerminalLocation,
) -> Option<&'a SessionSummary> {
    sessions.iter().find(|session| {
        session.terminal.as_ref().is_some_and(|terminal| {
            terminal.multiplexer == location.multiplexer && terminal.pane_id == location.pane_id
        })
    })
}

#[must_use]
pub fn started_session<'a, S: std::hash::BuildHasher>(
    sessions: &'a [SessionSummary],
    location: &TerminalLocation,
    known_sessions: &HashSet<SessionId, S>,
    cwd: &Path,
) -> Option<&'a SessionSummary> {
    if let Some(session) = session_at_location(sessions, location)
        .filter(|session| !known_sessions.contains(&session.id))
    {
        return Some(session);
    }
    let mut candidates = sessions.iter().filter(|session| {
        !known_sessions.contains(&session.id)
            && session.terminal.is_none()
            && session
                .cwd
                .as_deref()
                .is_some_and(|value| Path::new(value) == cwd)
    });
    let candidate = candidates.next()?;
    candidates.next().is_none().then_some(candidate)
}

fn sessions_by_pane(
    sessions: &[SessionSummary],
) -> HashMap<(agentix_domain::MultiplexerKind, String), SessionId> {
    sessions
        .iter()
        .filter_map(|session| {
            session.terminal.as_ref().map(|terminal| {
                (
                    (terminal.multiplexer, terminal.pane_id.clone()),
                    session.id.clone(),
                )
            })
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

fn validate_name(name: &str) -> Result<(), MultiplexerError> {
    if !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        Ok(())
    } else {
        Err(MultiplexerError::InvalidName)
    }
}

fn resolve_workspace(value: &str) -> Result<PathBuf, MultiplexerError> {
    let path = if value == "~" {
        dirs::home_dir().ok_or_else(|| MultiplexerError::InvalidWorkspace(value.into()))?
    } else if let Some(suffix) = value.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or_else(|| MultiplexerError::InvalidWorkspace(value.into()))?
            .join(suffix)
    } else {
        PathBuf::from(value)
    };
    if !path.is_dir() {
        return Err(MultiplexerError::InvalidWorkspace(
            path.display().to_string(),
        ));
    }
    std::fs::canonicalize(&path)
        .map_err(|_| MultiplexerError::InvalidWorkspace(path.display().to_string()))
}

pub fn is_shell_command(command: &str) -> bool {
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

pub fn command_basename(command: &str) -> String {
    Path::new(command)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(command)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentix_domain::SessionStatus;
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
    fn workspace_must_be_a_directory() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(resolve_workspace(file.path().to_str().unwrap()).is_err());
    }

    #[test]
    fn known_terminal_cannot_fall_back_to_another_multiplexer() {
        let location = TerminalLocation {
            multiplexer: MultiplexerKind::Tmux,
            session: "work".into(),
            window_index: "0".into(),
            window_name: "agent".into(),
            pane_index: "0".into(),
            pane_id: "%1".into(),
        };
        let mut other = location.clone();
        other.multiplexer = MultiplexerKind::Rmux;
        let sessions = [SessionSummary {
            id: SessionId::new("other"),
            name: None,
            preview: None,
            cwd: Some("/work".into()),
            updated_at: None,
            status: SessionStatus::Active,
            terminal: Some(other),
        }];
        assert!(
            started_session(&sessions, &location, &HashSet::new(), Path::new("/work")).is_none()
        );
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
    fn converts_driver_inventory_into_the_ui_hierarchy() {
        let base = PaneState {
            multiplexer: MultiplexerKind::Rmux,
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
            PaneState {
                pane_id: "%2".into(),
                pane_index: 1,
                active: false,
                current_command: "fish".into(),
                foreground_pid: Some(43),
                ..base
            },
        ];
        let sessions_by_terminal = HashMap::from([(
            (MultiplexerKind::Rmux, "%1".into()),
            SessionId::new("thr_agentix"),
        )]);

        let snapshot = snapshot_from_inventory(&inventory, &sessions_by_terminal);

        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].windows[0].panes.len(), 2);
        assert!(snapshot.sessions[0].windows[0].panes[0].active);
        assert_eq!(
            snapshot.sessions[0].windows[0].panes[0].agent_session,
            Some(SessionId::new("thr_agentix"))
        );
    }

    #[test]
    fn finds_only_the_session_running_in_the_created_pane() {
        let target = TerminalLocation {
            multiplexer: MultiplexerKind::Rmux,
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
            multiplexer: MultiplexerKind::Rmux,
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
