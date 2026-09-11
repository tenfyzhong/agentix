use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use agentix_domain::{SessionId, SessionStatus, SessionSummary, TerminalLocation};
use thiserror::Error;

use crate::{CodexEndpoint, multiplexer::rmux_process_locations};

#[derive(Debug, Error)]
pub(crate) enum ProcessDiscoveryError {
    #[error("failed to inspect running Codex processes: {0}")]
    Io(#[from] std::io::Error),
    #[error("process inspection command failed: {0}")]
    Command(String),
    #[error("failed to inspect rmux panes: {0}")]
    Rmux(String),
    #[error("the Codex daemon process could not be identified")]
    DaemonNotFound,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexProcessDiscovery {
    codex_home: PathBuf,
    socket_path: PathBuf,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RunningProcessSnapshot {
    pub(crate) direct_session_ids: HashSet<SessionId>,
    pub(crate) direct_terminal_locations: HashMap<SessionId, TerminalLocation>,
    pub(crate) daemon_clients: Vec<DaemonClient>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DaemonClient {
    pub(crate) pid: u32,
    pub(crate) cwd: PathBuf,
    pub(crate) terminal: Option<TerminalLocation>,
}

impl CodexProcessDiscovery {
    pub(crate) fn for_endpoint(endpoint: &CodexEndpoint) -> Option<Self> {
        endpoint.codex_home().map(|codex_home| Self {
            codex_home: codex_home.to_owned(),
            socket_path: endpoint.socket_path().to_owned(),
        })
    }

    pub(crate) async fn discover(&self) -> Result<RunningProcessSnapshot, ProcessDiscoveryError> {
        let interactive = interactive_codex_processes()?;
        let daemon_pid = daemon_pid(&self.socket_path)?;
        let lock_owners = writer_lock_owners(&self.codex_home)?;
        let panes = rmux_process_locations()
            .await
            .map_err(|error| ProcessDiscoveryError::Rmux(error.to_string()))?;
        let direct_session_ids = lock_owners
            .iter()
            .filter(|(_, pid)| **pid != daemon_pid && interactive.contains_key(pid))
            .map(|(session_id, _)| session_id.clone())
            .collect::<HashSet<_>>();
        let direct_terminal_locations = lock_owners
            .iter()
            .filter(|(_, pid)| **pid != daemon_pid)
            .filter_map(|(session_id, pid)| {
                interactive
                    .get(pid)
                    .and_then(|_| panes.get(pid))
                    .cloned()
                    .map(|terminal| (session_id.clone(), terminal))
            })
            .collect();
        let direct_pids = lock_owners
            .values()
            .filter(|pid| **pid != daemon_pid && interactive.contains_key(pid))
            .copied()
            .collect::<HashSet<_>>();
        let daemon_client_pids = interactive
            .keys()
            .filter(|pid| !direct_pids.contains(pid))
            .copied()
            .collect::<HashSet<_>>();
        let process_cwds = process_cwds(&daemon_client_pids)?;
        let daemon_clients = daemon_client_pids
            .iter()
            .filter_map(|pid| {
                process_cwds.get(pid).cloned().map(|cwd| DaemonClient {
                    pid: *pid,
                    cwd,
                    terminal: interactive.get(pid).and_then(|_| panes.get(pid)).cloned(),
                })
            })
            .collect();
        Ok(RunningProcessSnapshot {
            direct_session_ids,
            direct_terminal_locations,
            daemon_clients,
        })
    }
}

pub(crate) struct RunningSessionSelection {
    pub ids: HashSet<SessionId>,
    pub terminals: HashMap<SessionId, TerminalLocation>,
    clients: HashMap<u32, SessionId>,
}

#[derive(Default)]
pub(crate) struct RunningSessionResolver {
    bindings: HashMap<u32, SessionId>,
}

impl RunningSessionResolver {
    pub(crate) fn resolve(
        &mut self,
        loaded: &[SessionSummary],
        snapshot: &RunningProcessSnapshot,
    ) -> RunningSessionSelection {
        let loaded_ids = loaded
            .iter()
            .map(|session| &session.id)
            .collect::<HashSet<_>>();
        let clients = snapshot
            .daemon_clients
            .iter()
            .map(|client| client.pid)
            .collect::<HashSet<_>>();
        self.bindings.retain(|pid, session| {
            clients.contains(pid)
                && loaded_ids.contains(session)
                && !snapshot.direct_session_ids.contains(session)
        });
        let mut unresolved = RunningProcessSnapshot {
            direct_session_ids: snapshot.direct_session_ids.clone(),
            direct_terminal_locations: snapshot.direct_terminal_locations.clone(),
            daemon_clients: Vec::new(),
        };
        for client in &snapshot.daemon_clients {
            if let Some(session) = self.bindings.get(&client.pid) {
                unresolved.direct_session_ids.insert(session.clone());
                if let Some(terminal) = &client.terminal {
                    unresolved
                        .direct_terminal_locations
                        .insert(session.clone(), terminal.clone());
                }
            } else {
                unresolved.daemon_clients.push(client.clone());
            }
        }
        let selection = resolve_running_sessions(loaded, &unresolved);
        for (pid, session) in &selection.clients {
            self.bindings.insert(*pid, session.clone());
        }
        selection
    }
}

pub(crate) fn resolve_running_sessions(
    loaded: &[SessionSummary],
    snapshot: &RunningProcessSnapshot,
) -> RunningSessionSelection {
    let mut result = RunningSessionSelection {
        ids: snapshot.direct_session_ids.clone(),
        terminals: snapshot.direct_terminal_locations.clone(),
        clients: HashMap::new(),
    };
    if snapshot.daemon_clients.is_empty() {
        return result;
    }
    let mut clients_by_cwd = HashMap::<&Path, Vec<&DaemonClient>>::new();
    for client in &snapshot.daemon_clients {
        clients_by_cwd.entry(&client.cwd).or_default().push(client);
    }
    let mut sessions_by_cwd = HashMap::<&Path, Vec<&SessionSummary>>::new();
    for session in loaded {
        #[cfg(test)]
        tests::SESSION_VISITS.set(tests::SESSION_VISITS.get() + 1);
        if snapshot.direct_session_ids.contains(&session.id) {
            continue;
        }
        if let Some(cwd) = session.cwd.as_deref().map(Path::new)
            && clients_by_cwd.contains_key(cwd)
        {
            sessions_by_cwd.entry(cwd).or_default().push(session);
        }
    }
    let priority = |left: &&SessionSummary, right: &&SessionSummary| {
        session_priority(right)
            .cmp(&session_priority(left))
            .then_with(|| right.id.as_str().cmp(left.id.as_str()))
    };
    for (cwd, mut clients) in clients_by_cwd {
        let Some(mut candidates) = sessions_by_cwd.remove(cwd) else {
            continue;
        };
        clients.sort_by_key(|client| std::cmp::Reverse(client.pid));
        if candidates.len() > clients.len() {
            // Keep only the highest-priority candidates before ordering their terminals.
            candidates.select_nth_unstable_by(clients.len(), priority);
            candidates.truncate(clients.len());
        }
        candidates.sort_by(priority);
        for (session, client) in candidates.into_iter().zip(clients) {
            result.ids.insert(session.id.clone());
            result.clients.insert(client.pid, session.id.clone());
            // A client without a terminal still occupies its priority slot.
            if let Some(terminal) = &client.terminal {
                result
                    .terminals
                    .insert(session.id.clone(), terminal.clone());
            }
        }
    }
    result
}

pub(crate) fn confirm_exited_sessions(
    subscriptions: &HashSet<SessionId>,
    running: &HashSet<SessionId>,
    missing_counts: &mut HashMap<SessionId, u8>,
) -> Vec<SessionId> {
    missing_counts.retain(|session, _| subscriptions.contains(session));
    let mut exited = Vec::new();
    for session in subscriptions {
        if running.contains(session) {
            missing_counts.remove(session);
            continue;
        }
        let count = missing_counts.entry(session.clone()).or_default();
        *count = count.saturating_add(1);
        if *count >= 2 {
            exited.push(session.clone());
        }
    }
    exited.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    exited
}

pub(crate) fn reappeared_sessions(
    exited: &HashSet<SessionId>,
    running: &HashSet<SessionId>,
) -> Vec<SessionId> {
    let mut resumed = exited.intersection(running).cloned().collect::<Vec<_>>();
    resumed.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    resumed
}

fn session_priority(session: &SessionSummary) -> (bool, Option<i64>) {
    (session.status == SessionStatus::Active, session.updated_at)
}

fn interactive_codex_processes() -> Result<HashMap<u32, String>, ProcessDiscoveryError> {
    let output = run(Command::new("ps").args(["-axo", "pid=,tty=,comm=,args="]))?;
    Ok(parse_codex_processes(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn daemon_pid(socket_path: &Path) -> Result<u32, ProcessDiscoveryError> {
    let output = run(Command::new("lsof").arg("-t").arg(socket_path))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().parse().ok())
        .ok_or(ProcessDiscoveryError::DaemonNotFound)
}

fn writer_lock_owners(codex_home: &Path) -> Result<HashMap<SessionId, u32>, ProcessDiscoveryError> {
    let lock_dir = codex_home.join("thread-writer-locks");
    let lock_paths = match std::fs::read_dir(lock_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension() == Some(OsStr::new("lock"))
                    && path
                        .file_name()
                        .and_then(OsStr::to_str)
                        .is_some_and(|name| !name.starts_with('.'))
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    if lock_paths.is_empty() {
        return Ok(HashMap::new());
    }
    let mut command = Command::new("lsof");
    command.args(["-n", "-Fpn"]);
    command.args(lock_paths);
    let output = command.output()?;
    Ok(parse_lock_owners(&String::from_utf8_lossy(&output.stdout)))
}

fn process_cwds(pids: &HashSet<u32>) -> Result<HashMap<u32, PathBuf>, ProcessDiscoveryError> {
    if pids.is_empty() {
        return Ok(HashMap::new());
    }
    let pid_list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let output = Command::new("lsof")
        .args(["-n", "-a", "-d", "cwd", "-Fpn", "-p"])
        .arg(pid_list)
        .output()?;
    Ok(parse_process_paths(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn run(command: &mut Command) -> Result<Output, ProcessDiscoveryError> {
    let description = format!("{command:?}");
    let output = command.output()?;
    if output.status.success() {
        return Ok(output);
    }
    Err(ProcessDiscoveryError::Command(format!(
        "{description} exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn parse_codex_processes(output: &str) -> HashMap<u32, String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let tty = fields.next()?;
            let command = fields.next()?;
            (tty != "?"
                && tty != "??"
                && tty != "-"
                && Path::new(command).file_name() == Some(OsStr::new("codex")))
            .then(|| (pid, normalize_tty(tty).to_owned()))
        })
        .collect()
}

fn normalize_tty(tty: &str) -> &str {
    tty.strip_prefix("/dev/").unwrap_or(tty)
}

fn parse_lock_owners(output: &str) -> HashMap<SessionId, u32> {
    let mut pid = None;
    let mut owners = HashMap::new();
    for line in output.lines() {
        match line.as_bytes().first() {
            Some(b'p') => pid = line[1..].parse().ok(),
            Some(b'n') => {
                let path = Path::new(&line[1..]);
                if let (Some(pid), Some(session_id)) =
                    (pid, path.file_stem().and_then(OsStr::to_str))
                {
                    owners.insert(SessionId::new(session_id), pid);
                }
            }
            _ => {}
        }
    }
    owners
}

fn parse_process_paths(output: &str) -> HashMap<u32, PathBuf> {
    let mut pid = None;
    let mut paths = HashMap::new();
    for line in output.lines() {
        match line.as_bytes().first() {
            Some(b'p') => pid = line[1..].parse().ok(),
            Some(b'n') => {
                if let Some(pid) = pid {
                    paths.insert(pid, PathBuf::from(&line[1..]));
                }
            }
            _ => {}
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use agentix_domain::{SessionId, SessionStatus, SessionSummary, TerminalLocation};

    use super::{
        DaemonClient, RunningProcessSnapshot, confirm_exited_sessions, parse_codex_processes,
        parse_lock_owners, reappeared_sessions, resolve_running_sessions,
    };

    thread_local! {
        pub(super) static SESSION_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn concurrent_clients_keep_their_sessions_when_the_newer_client_exits() {
        let mut resolver = super::RunningSessionResolver::default();
        let a = terminal("a", "0", "shell", "0", "%1");
        let b = terminal("b", "0", "shell", "0", "%2");
        let mut snapshot = RunningProcessSnapshot {
            daemon_clients: vec![DaemonClient {
                pid: 1,
                cwd: "/work".into(),
                terminal: Some(a.clone()),
            }],
            ..RunningProcessSnapshot::default()
        };
        let mut loaded = vec![session("a", "/work", SessionStatus::Idle, 100)];
        assert_eq!(
            resolver.resolve(&loaded, &snapshot).ids,
            HashSet::from([SessionId::new("a")])
        );
        loaded.push(session("b", "/work", SessionStatus::Active, 200));
        snapshot.daemon_clients.push(DaemonClient {
            pid: 2,
            cwd: "/work".into(),
            terminal: Some(b.clone()),
        });
        let both = resolver.resolve(&loaded, &snapshot);
        assert_eq!(
            both.terminals,
            HashMap::from([
                (SessionId::new("a"), a.clone()),
                (SessionId::new("b"), b.clone())
            ])
        );
        // Activity changes must not swap process ownership or terminal locations.
        loaded[0].status = SessionStatus::Active;
        loaded[0].updated_at = Some(300);
        assert_eq!(
            resolver.resolve(&loaded, &snapshot).terminals,
            both.terminals
        );
        snapshot.daemon_clients.pop();
        loaded[1].updated_at = Some(400);
        let remaining = resolver.resolve(&loaded, &snapshot);
        assert_eq!(remaining.ids, HashSet::from([SessionId::new("a")]));
        assert_eq!(
            remaining.terminals,
            HashMap::from([(SessionId::new("a"), a)])
        );
        snapshot.daemon_clients.clear();
        assert!(resolver.resolve(&loaded, &snapshot).ids.is_empty());
    }

    #[test]
    fn resolver_preserves_sessions_across_directory_changes_and_releases_exited_clients() {
        let mut resolver = super::RunningSessionResolver::default();
        let loaded = vec![
            session("a", "/work", SessionStatus::Idle, 100),
            session("b", "/work", SessionStatus::Idle, 200),
            session("elsewhere", "/other", SessionStatus::Active, 300),
        ];
        let mut snapshot = RunningProcessSnapshot {
            daemon_clients: vec![
                DaemonClient {
                    pid: 1,
                    cwd: "/work".into(),
                    terminal: None,
                },
                DaemonClient {
                    pid: 2,
                    cwd: "/work".into(),
                    terminal: None,
                },
            ],
            ..RunningProcessSnapshot::default()
        };
        assert_eq!(resolver.resolve(&loaded, &snapshot).ids.len(), 2);
        snapshot.daemon_clients.remove(0);
        let moved = terminal("moved", "1", "shell", "0", "%3");
        snapshot.daemon_clients[0].terminal = Some(moved.clone());
        let remaining = resolver.resolve(&loaded, &snapshot);
        assert_eq!(remaining.ids, HashSet::from([SessionId::new("b")]));
        assert_eq!(
            remaining.terminals,
            HashMap::from([(SessionId::new("b"), moved)])
        );
        // Changing directories does not change the process or its current session.
        // A more active thread in the destination must not replace the binding.
        snapshot.daemon_clients[0].cwd = "/other".into();
        let changed = resolver.resolve(&loaded, &snapshot);
        assert_eq!(changed.ids, remaining.ids);
        assert_eq!(changed.terminals, remaining.terminals);
        snapshot.daemon_clients.clear();
        assert!(resolver.resolve(&loaded, &snapshot).ids.is_empty());
        snapshot.daemon_clients.push(DaemonClient {
            pid: 2,
            cwd: "/work".into(),
            terminal: None,
        });
        assert_eq!(
            resolver.resolve(&loaded[..1], &snapshot).ids,
            HashSet::from([SessionId::new("a")])
        );
    }

    #[test]
    fn resolver_does_not_assign_an_unobserved_exited_client_to_a_survivor() {
        let mut resolver = super::RunningSessionResolver::default();
        let snapshot = RunningProcessSnapshot {
            daemon_clients: vec![DaemonClient {
                pid: 1,
                cwd: "/work".into(),
                terminal: None,
            }],
            ..RunningProcessSnapshot::default()
        };
        let mut loaded = vec![session("a", "/work", SessionStatus::Idle, 100)];
        resolver.resolve(&loaded, &snapshot);
        // B starts and exits entirely between discovery polls, leaving its thread loaded.
        loaded.push(session("b", "/work", SessionStatus::Active, 200));
        assert_eq!(
            resolver.resolve(&loaded, &snapshot).ids,
            HashSet::from([SessionId::new("a")])
        );
    }

    #[test]
    fn process_resolution_does_not_rescan_sessions_for_each_directory() {
        let loaded: Vec<_> = (0..1000)
            .map(|i| {
                session(
                    &format!("s{i:04}"),
                    &format!("/work/{}", i % 100),
                    if i % 3 == 0 {
                        SessionStatus::Active
                    } else {
                        SessionStatus::Idle
                    },
                    i,
                )
            })
            .collect();
        let direct = SessionId::new("direct-not-loaded");
        let snapshot = RunningProcessSnapshot {
            direct_session_ids: HashSet::from([direct.clone()]),
            direct_terminal_locations: HashMap::from([(
                direct.clone(),
                terminal("direct", "0", "shell", "0", "%0"),
            )]),
            daemon_clients: (0..100)
                .map(|i| DaemonClient {
                    pid: i,
                    cwd: format!("/work/{i}").into(),
                    terminal: None,
                })
                .collect(),
        };
        SESSION_VISITS.set(0);
        let result = resolve_running_sessions(&loaded, &snapshot);
        let selected = result.ids;
        let locations = result.terminals;
        let visits = SESSION_VISITS.get();
        let mut expected: HashSet<_> = (0..100)
            .map(|i| SessionId::new(format!("s{:04}", i + 100 * (9 - i % 3))))
            .collect();
        expected.insert(direct);
        assert_eq!(selected, expected);
        assert_eq!(locations, snapshot.direct_terminal_locations);
        assert!(
            visits <= loaded.len(),
            "directory resolution visited {visits} sessions for {} inputs",
            loaded.len()
        );
    }

    #[test]
    fn process_resolution_preserves_priority_and_missing_terminal_slots() {
        let loaded = vec![
            session("direct", "/work", SessionStatus::Active, 999),
            session("idle", "/work", SessionStatus::Idle, 999),
            session("a", "/work", SessionStatus::Active, 100),
            session("z", "/work", SessionStatus::Active, 100),
            session("unmatched", "/elsewhere", SessionStatus::Active, 1000),
        ];
        let low = terminal("low", "0", "shell", "0", "%1");
        let middle = terminal("middle", "0", "shell", "0", "%5");
        let direct = terminal("direct", "0", "shell", "0", "%9");
        let snapshot = RunningProcessSnapshot {
            direct_session_ids: HashSet::from([SessionId::new("direct")]),
            direct_terminal_locations: HashMap::from([(SessionId::new("direct"), direct.clone())]),
            daemon_clients: vec![
                DaemonClient {
                    pid: 1,
                    cwd: "/work".into(),
                    terminal: Some(low.clone()),
                },
                DaemonClient {
                    pid: 9,
                    cwd: "/work".into(),
                    terminal: None,
                },
                DaemonClient {
                    pid: 5,
                    cwd: "/work".into(),
                    terminal: Some(middle.clone()),
                },
            ],
        };
        assert_eq!(
            resolve_running_sessions(&loaded, &snapshot).ids,
            ["direct", "z", "a", "idle"]
                .map(SessionId::new)
                .into_iter()
                .collect()
        );
        assert_eq!(
            resolve_running_sessions(&loaded, &snapshot).terminals,
            HashMap::from([
                (SessionId::new("direct"), direct),
                (SessionId::new("a"), middle),
                (SessionId::new("idle"), low)
            ])
        );
    }

    #[test]
    fn direct_only_discovery_skips_loaded_sessions() {
        let loaded = vec![session("unneeded", "/work", SessionStatus::Active, 100)];
        let snapshot = RunningProcessSnapshot {
            direct_session_ids: HashSet::from([SessionId::new("direct")]),
            ..RunningProcessSnapshot::default()
        };
        SESSION_VISITS.set(0);
        let result = resolve_running_sessions(&loaded, &snapshot);
        assert_eq!(result.ids, snapshot.direct_session_ids);
        assert_eq!(SESSION_VISITS.get(), 0);
    }

    #[test]
    fn process_snapshot_keeps_direct_sessions_and_matches_daemon_clients_by_cwd() {
        let loaded = vec![
            session("01a064f1-stale", "/stale", SessionStatus::Idle, 300),
            session("01a0656e-current", "/agentix", SessionStatus::Active, 200),
            session("older-same-cwd", "/agentix", SessionStatus::Idle, 100),
        ];
        let snapshot = RunningProcessSnapshot {
            direct_session_ids: HashSet::from([
                SessionId::new("01a062b2-direct"),
                SessionId::new("01a0619c-direct"),
                SessionId::new("01a060a7-direct"),
            ]),
            direct_terminal_locations: HashMap::new(),
            daemon_clients: vec![DaemonClient {
                pid: 32389,
                cwd: "/agentix".into(),
                terminal: Some(terminal("agentix", "1", "codex:agentix", "0", "%36")),
            }],
        };

        let selected = resolve_running_sessions(&loaded, &snapshot).ids;

        assert_eq!(
            selected,
            HashSet::from([
                SessionId::new("01a0656e-current"),
                SessionId::new("01a062b2-direct"),
                SessionId::new("01a0619c-direct"),
                SessionId::new("01a060a7-direct"),
            ])
        );

        let locations = resolve_running_sessions(&loaded, &snapshot).terminals;
        assert_eq!(
            locations.get(&SessionId::new("01a0656e-current")),
            Some(&terminal("agentix", "1", "codex:agentix", "0", "%36"))
        );
    }

    #[test]
    fn running_session_monitor_confirms_exit_after_two_missing_snapshots() {
        let session = SessionId::new("thr_a");
        let subscriptions = HashSet::from([session.clone()]);
        let mut missing_counts = HashMap::new();

        assert!(
            confirm_exited_sessions(&subscriptions, &HashSet::new(), &mut missing_counts)
                .is_empty()
        );
        assert_eq!(
            confirm_exited_sessions(&subscriptions, &HashSet::new(), &mut missing_counts),
            vec![session.clone()]
        );

        let running = HashSet::from([session.clone()]);
        assert!(confirm_exited_sessions(&subscriptions, &running, &mut missing_counts).is_empty());
        assert!(
            confirm_exited_sessions(&subscriptions, &HashSet::new(), &mut missing_counts)
                .is_empty()
        );
    }

    #[test]
    fn running_session_monitor_detects_an_exited_session_reappearing() {
        let session = SessionId::new("thr_a");
        let exited = HashSet::from([session.clone()]);
        let running = HashSet::from([session.clone(), SessionId::new("thr_b")]);

        assert_eq!(reappeared_sessions(&exited, &running), vec![session]);
    }

    #[test]
    fn parses_only_interactive_codex_processes() {
        let output = r"
39205 ttys004 codex codex
32389 ttys005 codex codex resume 01a0
12345 pts/7 codex codex
25052 ?? codex /opt/codex app-server --listen unix://
84418 ?? codex /Applications/Codex app-server
34538 ?? codex-code-mode-host /opt/codex-code-mode-host
";

        assert_eq!(
            parse_codex_processes(output),
            HashMap::from([
                (39205, "ttys004".into()),
                (32389, "ttys005".into()),
                (12345, "pts/7".into()),
            ])
        );
    }

    #[test]
    fn parses_open_writer_locks_by_process() {
        let output = r"
p21148
ccodex
f49
n/home/me/.codex/thread-writer-locks/01a062b2.lock
p25052
ccodex
f33
n/home/me/.codex/thread-writer-locks/01a064f1.lock
f61
n/home/me/.codex/thread-writer-locks/01a0656e.lock
";

        assert_eq!(
            parse_lock_owners(output),
            HashMap::from([
                (SessionId::new("01a062b2"), 21148),
                (SessionId::new("01a064f1"), 25052),
                (SessionId::new("01a0656e"), 25052),
            ])
        );
    }

    fn session(id: &str, cwd: &str, status: SessionStatus, updated_at: i64) -> SessionSummary {
        SessionSummary {
            id: SessionId::new(id),
            name: None,
            preview: None,
            cwd: Some(cwd.into()),
            updated_at: Some(updated_at),
            status,
            terminal: None,
        }
    }

    fn terminal(
        session: &str,
        window_index: &str,
        window_name: &str,
        pane_index: &str,
        pane_id: &str,
    ) -> TerminalLocation {
        TerminalLocation {
            session: session.into(),
            window_index: window_index.into(),
            window_name: window_name.into(),
            pane_index: pane_index.into(),
            pane_id: pane_id.into(),
        }
    }
}
