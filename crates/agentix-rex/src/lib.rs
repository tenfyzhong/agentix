//! Rex CLI adapter for local terminal sessions.
mod screen;
use agentix_multiplexer::MultiplexerError;
use screen::decode_vt;
use std::path::{Path, PathBuf};

fn executable_candidates(path: Option<&str>, home: Option<&Path>, macos: bool) -> Vec<PathBuf> {
    let mut candidates: Vec<_> = path
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join("rex"))
        .collect();
    if macos {
        let roots = std::iter::once(PathBuf::from("/Applications"))
            .chain(home.map(|home| home.join("Applications")));
        for root in roots {
            for app in ["Rex.app", "Rex Beta.app"] {
                candidates.push(
                    root.join(app)
                        .join("Contents/Helpers/Rex Server.rex/Contents/MacOS/rex"),
                );
            }
        }
    }
    candidates
}

fn validate_endpoint(endpoint: &str) -> Result<(), MultiplexerError> {
    let url =
        url::Url::parse(endpoint).map_err(|error| MultiplexerError::Backend(error.to_string()))?;
    if !endpoint.starts_with("unix:///")
        || url.scheme() != "unix"
        || url.host_str().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() == "/"
    {
        return Err(MultiplexerError::Backend(
            "Rex requires a local absolute Unix socket endpoint".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_rejects_labels_that_would_be_cli_flags() {
        let prepared = PreparedMutation {
            mutation: agentix_domain::MultiplexerMutation {
                target: MultiplexerTarget::NewSession {
                    name: "--help".into(),
                    cwd: "/tmp".into(),
                },
                launch_agent: false,
            },
            cwd: "/tmp".into(),
        };
        assert!(creation_args(&prepared, None, None).is_err());
    }

    #[test]
    fn executable_candidates_prefer_path_then_standard_macos_apps() {
        let paths = executable_candidates(
            Some("/bin:/custom"),
            Some(std::path::Path::new("/home/test")),
            true,
        );
        assert_eq!(paths[0], std::path::PathBuf::from("/bin/rex"));
        assert_eq!(paths[1], std::path::PathBuf::from("/custom/rex"));
        assert_eq!(
            paths[2],
            std::path::PathBuf::from(
                "/Applications/Rex.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex"
            )
        );
        assert_eq!(paths.len(), 6);
        assert_eq!(executable_candidates(Some("/bin"), None, false).len(), 1);
    }

    #[test]
    fn server_endpoint_must_be_a_local_absolute_unix_socket() {
        assert!(validate_endpoint("unix:///tmp/rex%20server.sock").is_ok());
        for invalid in [
            "tcp://remote:123",
            "unix://remote/tmp/rex.sock",
            "unix:relative",
            "",
        ] {
            assert!(validate_endpoint(invalid).is_err(), "{invalid}");
        }
    }
}

#[cfg(all(test, unix))]
mod client_tests {
    use super::*;
    use agentix_multiplexer::MultiplexerDriver;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn probe_and_inventory_pin_the_local_endpoint_without_autostart() {
        let directory = tempfile::tempdir().unwrap();
        let command = directory.path().join("rex");
        let log = directory.path().join("calls");
        std::fs::write(&command, format!(r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
case "$*" in
*whoami*) printf '%s' '{{"endpoint":"unix:///tmp/rex.sock","host_id":"local"}}';;
*'ls --json'*) printf '%s' '{{"sessions":[{{"session_id":"session:one","label":"work"}}]}}';;
*'session inspect'*) printf '%s' '{{"session_id":"session:one","label":"work","windows":[{{"window_id":"window:one","label":"agent","focused_block_id":"block:one","layers":[{{"blocks":[{{"block_id":"block:one","creator_name":"com.superlogical.terminal","host_id":"local"}},{{"block_id":"block:remote","creator_name":"com.superlogical.terminal","host_id":"remote"}}]}}]}}]}}';;
*process*) printf '%s' '{{"child":{{"pid":1}},"foreground":{{"pid":42,"name":"fish","cwd":"/tmp"}},"last_exit":null}}';;
*) exit 1;;
esac
"#,log.display())).unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
        let driver = RexDriver::with_command(command);
        assert!(driver.probe().await.unwrap());
        let panes = driver.inventory(false).await.unwrap().unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, "block:one");
        assert_eq!(panes[0].foreground_pid, Some(42));
        assert_eq!(panes[0].window_index, 1);
        assert_eq!(panes[0].current_command, "fish");
        let calls = std::fs::read_to_string(log).unwrap();
        assert!(calls.lines().all(|line| line.contains("--autostart=false")));
        assert!(
            calls
                .lines()
                .skip(1)
                .all(|line| line.contains("-S unix:///tmp/rex.sock"))
        );
        assert!(!calls.contains("-b block:remote"));
    }
}

use agentix_domain::{MultiplexerKind, MultiplexerTarget, PaneSplitDirection};
use agentix_multiplexer::TerminalControl;
use agentix_multiplexer::{
    MultiplexerDriver, MultiplexerOutcome, PaneState, PreparedMutation, terminal_location,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{process::Command, sync::OnceCell};

#[derive(Debug)]
struct Connection {
    command: PathBuf,
    endpoint: String,
    host: String,
}
#[derive(Debug, Default)]
pub struct RexDriver {
    command: Option<PathBuf>,
    connection: OnceCell<Connection>,
}
fn error(message: impl std::fmt::Display) -> MultiplexerError {
    MultiplexerError::Backend(message.to_string())
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, MultiplexerError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error(format!("Missing Rex {key}")))
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, MultiplexerError> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| error(format!("Missing Rex {key}")))
}
impl RexDriver {
    #[must_use]
    pub fn with_command(command: PathBuf) -> Self {
        Self {
            command: Some(command),
            connection: OnceCell::new(),
        }
    }
    async fn output(
        command: &Path,
        endpoint: Option<&str>,
        args: &[String],
    ) -> Result<Value, MultiplexerError> {
        let mut process = Command::new(command);
        process.args(["--autostart=false", "--timeout", "4s"]);
        if let Some(endpoint) = endpoint {
            process.args(["-S", endpoint]);
        }
        process
            .args(args)
            .env_remove("REX_SERVER")
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK")
            .env_remove("REX_CLIENT")
            .stdin(std::process::Stdio::null());
        let result = agentix_multiplexer::terminal_command_output(&mut process).await?;
        if !result.status.success() {
            return Err(error(format!(
                "Rex command failed: {}",
                String::from_utf8_lossy(&result.stderr)
            )));
        }
        if result.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&result.stdout).map_err(error)
    }
    async fn connection(&self) -> Result<&Connection, MultiplexerError> {
        self.connection
            .get_or_try_init(|| async {
                let command = if let Some(command) = &self.command {
                    command.clone()
                } else {
                    executable_candidates(
                        std::env::var("PATH").ok().as_deref(),
                        dirs::home_dir().as_deref(),
                        cfg!(target_os = "macos"),
                    )
                    .into_iter()
                    .find(|path| {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            path.metadata()
                                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                        }
                        #[cfg(not(unix))]
                        {
                            path.is_file()
                        }
                    })
                    .ok_or_else(|| error("Rex executable was not found"))?
                };
                let identity =
                    Self::output(&command, None, &["whoami".into(), "--json".into()]).await?;
                let endpoint = field(&identity, "endpoint")?.to_owned();
                validate_endpoint(&endpoint)?;
                Ok(Connection {
                    command,
                    endpoint,
                    host: field(&identity, "host_id")?.into(),
                })
            })
            .await
    }
    async fn call(&self, args: &[String]) -> Result<Value, MultiplexerError> {
        let connection = self.connection().await?;
        Self::output(&connection.command, Some(&connection.endpoint), args).await
    }
    pub async fn probe(&self) -> Result<bool, MultiplexerError> {
        self.connection().await?;
        array(
            &self.call(&["ls".into(), "--json".into()]).await?,
            "sessions",
        )?;
        Ok(true)
    }
    async fn block(
        &self,
        session: &str,
        block: &str,
        method: &str,
        args: Value,
    ) -> Result<Value, MultiplexerError> {
        self.call(&[
            "block".into(),
            "call".into(),
            "-s".into(),
            session.into(),
            "-b".into(),
            block.into(),
            method.into(),
            args.to_string(),
        ])
        .await
    }
    async fn panes(&self) -> Result<Vec<PaneState>, MultiplexerError> {
        let host = &self.connection().await?.host;
        let sessions = self.call(&["ls".into(), "--json".into()]).await?;
        let mut panes = Vec::new();
        for session in array(&sessions, "sessions")? {
            let id = field(session, "session_id")?;
            let view = self
                .call(&[
                    "session".into(),
                    "inspect".into(),
                    id.into(),
                    "--json".into(),
                ])
                .await?;
            for (wi, window) in array(&view, "windows")?.iter().enumerate() {
                let mut pi = 0;
                for layer in array(window, "layers")? {
                    for block in array(layer, "blocks")? {
                        if block["creator_name"] != "com.superlogical.terminal"
                            || block["host_id"].as_str() != Some(host)
                        {
                            continue;
                        }
                        let bid = field(block, "block_id")?;
                        let process = self.block(id, bid, "process", json!({})).await?;
                        let foreground = &process["foreground"];
                        pi += 1;
                        panes.push(PaneState {
                            multiplexer: MultiplexerKind::Rex,
                            session_id: id.into(),
                            session_name: view["label"].as_str().unwrap_or(id).into(),
                            window_id: field(window, "window_id")?.into(),
                            window_index: u32::try_from(wi + 1).map_err(error)?,
                            window_name: window["label"].as_str().unwrap_or("").into(),
                            pane_id: bid.into(),
                            pane_index: pi,
                            active: window["focused_block_id"] == bid,
                            current_command: foreground["name"].as_str().unwrap_or("").into(),
                            cwd: foreground["cwd"]
                                .as_str()
                                .or_else(|| process["child"]["cwd"].as_str())
                                .unwrap_or("")
                                .into(),
                            foreground_pid: foreground["pid"]
                                .as_u64()
                                .and_then(|pid| u32::try_from(pid).ok()),
                        });
                    }
                }
            }
        }
        Ok(panes)
    }
}
#[async_trait]
impl MultiplexerDriver for RexDriver {
    async fn codex_terminal_input(
        &self,
        pid: u32,
        clear: Option<&str>,
    ) -> Result<Option<String>, MultiplexerError> {
        let terminal = self.terminal(pid).await?;
        agentix_multiplexer::codex_terminal_input_with(&terminal, pid, clear).await
    }
    async fn new_codex_session(
        &self,
        pid: u32,
    ) -> Result<agentix_multiplexer::CodexNewSessionOutcome, MultiplexerError> {
        let terminal = self.terminal(pid).await?;
        agentix_multiplexer::send_codex_new_with(&terminal, pid).await
    }
    async fn exit_codex_session(&self, pid: u32) -> Result<(), MultiplexerError> {
        let terminal = self.terminal(pid).await?;
        agentix_multiplexer::send_codex_exit_with(&terminal, pid).await
    }
    async fn select_codex_checkout(
        &self,
        pid: u32,
        choice: agentix_multiplexer::CodexCheckoutChoice,
    ) -> Result<(), MultiplexerError> {
        let terminal = self.terminal(pid).await?;
        agentix_multiplexer::select_codex_checkout_with(&terminal, pid, choice).await
    }
    async fn inspect_interaction(
        &self,
        agent: agentix_domain::AgentKind,
        pid: u32,
    ) -> Result<Option<agentix_domain::TerminalInteraction>, MultiplexerError> {
        let terminal = self.terminal(pid).await?;
        agentix_multiplexer::inspect_terminal_interaction_with(
            &terminal,
            &terminal.block,
            pid,
            agent,
        )
        .await
    }
    async fn respond_interaction(
        &self,
        agent: agentix_domain::AgentKind,
        pid: u32,
        expected: &agentix_domain::TerminalInteraction,
        response: agentix_domain::TerminalInteractionResponse,
    ) -> Result<(), MultiplexerError> {
        let terminal = self.terminal(pid).await?;
        agentix_multiplexer::respond_terminal_interaction_with(
            &terminal,
            &terminal.block,
            pid,
            agent,
            expected,
            response,
        )
        .await
    }
    async fn process_locations(
        &self,
    ) -> Result<std::collections::HashMap<u32, agentix_domain::TerminalLocation>, MultiplexerError>
    {
        let mut roots = std::collections::HashMap::new();
        for pane in self.panes().await? {
            let process = self
                .block(&pane.session_id, &pane.pane_id, "process", json!({}))
                .await?;
            if let Some(root) = process["child"]["pid"]
                .as_u64()
                .and_then(|p| u32::try_from(p).ok())
            {
                roots.insert(root, terminal_location(&pane));
            }
        }
        if roots.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut command = Command::new("ps");
        command.args(["-A", "-o", "pid=,ppid="]);
        let output = agentix_multiplexer::terminal_command_output(&mut command).await?;
        if !output.status.success() {
            return Err(error("Cannot read local process ancestry"));
        }
        Ok(descendants(
            &roots,
            &String::from_utf8_lossy(&output.stdout),
        ))
    }
    fn kind(&self) -> MultiplexerKind {
        MultiplexerKind::Rex
    }
    async fn inventory(&self, _start: bool) -> Result<Option<Vec<PaneState>>, MultiplexerError> {
        self.panes().await.map(Some)
    }
    async fn execute(
        &self,
        prepared: &PreparedMutation,
        argv: Option<&[String]>,
    ) -> Result<MultiplexerOutcome, MultiplexerError> {
        self.execute_inner(prepared, argv).await
    }
}

#[cfg(test)]
mod screen_tests {
    use super::*;
    #[test]
    fn vt_snapshot_preserves_wide_cells_background_dim_and_cursor() {
        let snapshot = decode_vt("old\r\n\x1b[48;5;235m\x1b[8X\r\n› \x1b[2m中文\x1b[22m\x1b[2X\r\n\x1b[8X\x1b[0m\x1b[3;3H", 4, 8).unwrap();
        assert_eq!((snapshot.1, snapshot.2), (2, 2));
        assert!(snapshot.0.contains("\x1b[2m"));
        assert!(snapshot.0.contains("中文"));
        for malformed in ["\x1b[", "\x1b[999;1H", "\x1b]unsupported"] {
            assert!(decode_vt(malformed, 4, 8).is_err());
        }
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;
    #[test]
    fn creation_arguments_preserve_ids_directories_and_agent_arguments() {
        let target = MultiplexerTarget::NewWindow {
            session_id: "session:one".into(),
            name: "agent".into(),
            cwd: "/tmp/中文 dir".into(),
        };
        let prepared = PreparedMutation {
            mutation: agentix_domain::MultiplexerMutation {
                target,
                launch_agent: true,
            },
            cwd: "/tmp/中文 dir".into(),
        };
        let args = creation_args(&prepared, Some(&["codex".into(), "a;b".into()]), None).unwrap();
        assert!(args.windows(2).any(|v| v == ["-s", "session:one"]));
        assert!(args.windows(2).any(|v| v == ["-c", "/tmp/中文 dir"]));
        assert!(args.contains(&"--focus=false".into()));
        assert!(args.contains(&"--shell=none".into()));
        assert!(args.iter().any(|v| v.contains("a;b")));
    }
}

fn creation_args(
    prepared: &PreparedMutation,
    argv: Option<&[String]>,
    session: Option<&str>,
) -> Result<Vec<String>, MultiplexerError> {
    if let MultiplexerTarget::NewSession { name, .. } | MultiplexerTarget::NewWindow { name, .. } =
        &prepared.mutation.target
        && name.starts_with('-')
    {
        return Err(error(
            "Rex session and window names must not start with '-'",
        ));
    }
    let mut command_args: Vec<String> = match &prepared.mutation.target {
        MultiplexerTarget::NewSession { name, .. } => {
            vec!["new".into(), name.clone(), "--window".into(), name.clone()]
        }
        MultiplexerTarget::NewWindow {
            session_id, name, ..
        } => vec![
            "window".into(),
            "new".into(),
            "-s".into(),
            session_id.clone(),
            name.clone(),
            "--focus=false".into(),
        ],
        MultiplexerTarget::SplitPane {
            pane_id, direction, ..
        } => vec![
            "split".into(),
            "-s".into(),
            session.ok_or_else(|| error("Missing Rex session"))?.into(),
            "-b".into(),
            pane_id.clone(),
            format!(
                "--split={}",
                match direction {
                    PaneSplitDirection::Horizontal => "right",
                    PaneSplitDirection::Vertical => "below",
                }
            ),
            "--focus=false".into(),
        ],
        MultiplexerTarget::ExistingPane { .. } => {
            return Err(error("Existing pane is not a creation target"));
        }
    };
    command_args.extend([
        "-c".into(),
        prepared.cwd.to_string_lossy().into_owned(),
        "--json".into(),
        "--shell=none".into(),
        "--".into(),
    ]);
    command_args.extend(argv.map_or_else(
        || agentix_multiplexer::interactive_login_shell_argv().unwrap_or_default(),
        agentix_multiplexer::persistent_launch_argv,
    ));
    Ok(command_args)
}
impl RexDriver {
    async fn execute_inner(
        &self,
        prepared: &PreparedMutation,
        argv: Option<&[String]>,
    ) -> Result<MultiplexerOutcome, MultiplexerError> {
        let panes = self.panes().await?;
        let original = match &prepared.mutation.target {
            MultiplexerTarget::ExistingPane { pane_id }
            | MultiplexerTarget::SplitPane { pane_id, .. } => Some(
                panes
                    .iter()
                    .find(|p| &p.pane_id == pane_id)
                    .ok_or_else(|| error("Rex pane no longer exists"))?,
            ),
            _ => None,
        };
        if let MultiplexerTarget::ExistingPane { .. } = &prepared.mutation.target {
            let pane = original.ok_or_else(|| error("Missing Rex pane"))?;
            let argv = argv.ok_or_else(|| error("An existing pane must launch an agent"))?;
            if !["bash", "dash", "fish", "ksh", "sh", "zsh"]
                .contains(&pane.current_command.as_str())
            {
                return Err(MultiplexerError::BusyPane {
                    pane_id: pane.pane_id.clone(),
                    command: pane.current_command.clone(),
                });
            }
            let pid = pane
                .foreground_pid
                .ok_or_else(|| error("Rex shell process is unavailable"))?;
            let terminal = RexTerminal {
                driver: self,
                session: pane.session_id.clone(),
                block: pane.pane_id.clone(),
                expected_command: pane.current_command.clone(),
            };
            terminal.send_key(pid, "C-c").await?;
            let quote = |s: &str| {
                let value = if pane.current_command == "fish" {
                    s.replace('\\', "\\\\")
                } else {
                    s.into()
                };
                format!("'{}'", value.replace('\'', "'\\''"))
            };
            let launch = agentix_multiplexer::persistent_launch_argv(argv)
                .iter()
                .map(|s| quote(s))
                .collect::<Vec<_>>()
                .join(" ");
            let text = format!("cd {}; {}", quote(&prepared.cwd.to_string_lossy()), launch);
            terminal.send_text(pid, &text).await?;
            terminal.send_key(pid, "Enter").await?;
            return Ok(MultiplexerOutcome {
                location: terminal_location(pane),
            });
        }
        let command_args = creation_args(prepared, argv, original.map(|p| p.session_id.as_str()))?;
        let result = self.call(&command_args).await?;
        let created = if matches!(
            prepared.mutation.target,
            MultiplexerTarget::NewSession { .. }
        ) {
            &result["initial_windows"][0]
        } else {
            &result
        };
        let bid = created["block_ids"][0]
            .as_str()
            .filter(|id| id.starts_with("block:"))
            .ok_or_else(|| {
                error("Rex creation returned no block ID; inspect Rex before retrying")
            })?;
        match self.panes().await?.iter().find(|p| p.pane_id == bid) {
            Some(pane) => Ok(MultiplexerOutcome {
                location: terminal_location(pane),
            }),
            None => Err(error(format!(
                "Created Rex block {bid} is unavailable; inspect Rex before retrying"
            ))),
        }
    }
}

struct RexTerminal<'a> {
    driver: &'a RexDriver,
    session: String,
    block: String,
    expected_command: String,
}
impl RexTerminal<'_> {
    async fn verify(&self, pid: u32) -> Result<String, MultiplexerError> {
        let process = self
            .driver
            .block(&self.session, &self.block, "process", json!({}))
            .await?;
        if !process["last_exit"].is_null() {
            return Err(error("Original Rex process exited"));
        }
        let root = process["child"]["pid"]
            .as_u64()
            .ok_or_else(|| error("Rex root process is unavailable"))?;
        let mut command = Command::new("ps");
        command.args(["-A", "-o", "pid=,ppid="]);
        let output = agentix_multiplexer::terminal_command_output(&mut command).await?;
        if !output.status.success() {
            return Err(error("Cannot verify Rex process ancestry"));
        }
        let parents: std::collections::HashMap<u64, u64> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut p = line.split_whitespace();
                Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
            })
            .collect();
        let mut current = u64::from(pid);
        let mut seen = std::collections::HashSet::new();
        while current != root && current != 0 && seen.insert(current) {
            current = parents.get(&current).copied().unwrap_or_default();
        }
        if current != root || !parents.contains_key(&u64::from(pid)) {
            return Err(error("Original agent does not belong to this Rex block"));
        }
        let command = agentix_multiplexer::verify_foreground(pid).await?;
        if command != self.expected_command {
            return Err(error("Original Rex process changed"));
        }
        Ok(command)
    }
}
#[async_trait]
impl agentix_multiplexer::TerminalControl for RexTerminal<'_> {
    async fn snapshot(
        &self,
        pid: u32,
    ) -> Result<agentix_multiplexer::TerminalSnapshot, MultiplexerError> {
        let command = self.verify(pid).await?;
        let size = self
            .driver
            .block(&self.session, &self.block, "size", json!({}))
            .await?;
        let rows = size["rows"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| error("Invalid Rex rows"))?;
        let cols = size["columns"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| error("Invalid Rex columns"))?;
        let mut result = self
            .driver
            .block(
                &self.session,
                &self.block,
                "format",
                json!({"format":"vt","extra_cursor":true,"max_bytes":65536}),
            )
            .await?;
        let mut content = field(&result, "content")?.to_string();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut offset = content.len();
        while let Some(id) = result["id"].as_u64() {
            if offset > 64 * 1024 * 1024 || tokio::time::Instant::now() >= deadline {
                return Err(error("Rex snapshot exceeded its read budget"));
            }
            result = self
                .driver
                .block(
                    &self.session,
                    &self.block,
                    "blob_read",
                    json!({"id":id,"offset":offset,"max_bytes":65536}),
                )
                .await?;
            let part = blob_data(&result)?;
            offset += part.len();
            content.push_str(part);
            if result["eof"] == true {
                break;
            }
        }
        let (screen, x, y) = decode_vt(&content, rows, cols)?;
        let again = self
            .driver
            .block(&self.session, &self.block, "size", json!({}))
            .await?;
        if size["rows"] != again["rows"] || size["columns"] != again["columns"] {
            return Err(error("Rex terminal resized during capture"));
        }
        self.verify(pid).await?;
        let mut plain = String::new();
        let mut rest = screen.as_str();
        while !rest.is_empty() {
            if let Some(s) = rest.strip_prefix("\x1b[") {
                let end = s
                    .find('m')
                    .ok_or_else(|| error("Invalid normalized style"))?;
                rest = &s[end + 1..];
            } else {
                let ch = rest.chars().next().unwrap();
                plain.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
        Ok(agentix_multiplexer::TerminalSnapshot {
            command,
            cursor_x: x,
            cursor_y: y,
            in_mode: false,
            dead: false,
            screen,
            plain_screen: plain,
        })
    }
    async fn send_key(&self, pid: u32, key: &str) -> Result<(), MultiplexerError> {
        self.verify(pid).await?;
        let key = match key {
            "C-c" => "ctrl+c",
            "Enter" => "enter",
            "Escape" => "escape",
            "Up" => "up",
            "Down" => "down",
            other => other,
        };
        self.driver
            .call(&[
                "send-key".into(),
                "-s".into(),
                self.session.clone(),
                "-b".into(),
                self.block.clone(),
                key.into(),
            ])
            .await?;
        Ok(())
    }
    async fn send_text(&self, pid: u32, text: &str) -> Result<(), MultiplexerError> {
        self.verify(pid).await?;
        self.driver
            .block(&self.session, &self.block, "write", json!({"data":text}))
            .await?;
        Ok(())
    }
}
impl RexDriver {
    async fn terminal(&self, pid: u32) -> Result<RexTerminal<'_>, MultiplexerError> {
        let panes = self.panes().await?;
        let location = self
            .process_locations()
            .await?
            .remove(&pid)
            .ok_or_else(|| error("Original agent Rex block is unavailable"))?;
        let pane = panes
            .iter()
            .find(|p| p.pane_id == location.pane_id)
            .ok_or_else(|| error("Original Rex block disappeared"))?;
        Ok(RexTerminal {
            driver: self,
            session: pane.session_id.clone(),
            block: pane.pane_id.clone(),
            expected_command: agentix_multiplexer::verify_foreground(pid).await?,
        })
    }
}

#[cfg(all(test, unix))]
mod live_tests {
    use super::*;
    use agentix_multiplexer::TerminalControl;
    struct SessionCleanup {
        command: PathBuf,
        endpoint: String,
        id: String,
    }
    impl Drop for SessionCleanup {
        fn drop(&mut self) {
            let _ = std::process::Command::new(&self.command)
                .args([
                    "--autostart=false",
                    "--timeout",
                    "4s",
                    "-S",
                    &self.endpoint,
                    "kill",
                    &self.id,
                ])
                .output();
        }
    }
    #[tokio::test]
    #[ignore = "runs a deterministic Codex terminal fixture on the local Rex service"]
    #[allow(clippy::too_many_lines)] // Keep the complete isolated lifecycle scenario together.
    async fn local_rex_codex_draft_new_checkout_and_exit() {
        let directory = tempfile::tempdir().unwrap();
        let node_output = std::process::Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        assert!(node_output.status.success());
        let node = PathBuf::from(String::from_utf8(node_output.stdout).unwrap().trim());
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let executable = bin.join("codex");
        std::fs::copy(&node, &executable).unwrap();
        let libraries = node
            .canonicalize()
            .unwrap()
            .parent()
            .unwrap()
            .join("../lib");
        if libraries.exists() {
            std::os::unix::fs::symlink(libraries, directory.path().join("lib")).unwrap();
        }
        let output = directory.path().join("input");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/agentix-bridge/tests/claude-rmux-host.mjs");
        let argv = vec![
            executable.to_string_lossy().into_owned(),
            fixture.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
            "draft\nsecond".into(),
            "codex".into(),
            "plain".into(),
            "checkout".into(),
        ];
        let driver = RexDriver::default();
        let prepared = PreparedMutation {
            mutation: agentix_domain::MultiplexerMutation {
                target: MultiplexerTarget::NewSession {
                    name: format!("agentix-codex-test-{}", std::process::id()),
                    cwd: directory.path().to_string_lossy().into_owned(),
                },
                launch_agent: true,
            },
            cwd: directory.path().into(),
        };
        let outcome = driver.execute(&prepared, Some(&argv)).await.unwrap();
        let pane = driver
            .panes()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.pane_id == outcome.location.pane_id)
            .unwrap();
        let connection = driver.connection().await.unwrap();
        let _cleanup = SessionCleanup {
            command: connection.command.clone(),
            endpoint: connection.endpoint.clone(),
            id: pane.session_id.clone(),
        };
        let pid = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(pid) = driver
                    .panes()
                    .await
                    .unwrap()
                    .iter()
                    .find(|p| p.pane_id == pane.pane_id && p.current_command == "codex")
                    .and_then(|p| p.foreground_pid)
                    && driver
                        .codex_terminal_input(pid, None)
                        .await
                        .ok()
                        .flatten()
                        .as_deref()
                        == Some("draft\nsecond")
                {
                    break pid;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("fixture composer must become ready");
        assert!(driver.new_codex_session(pid).await.is_err());
        assert_eq!(
            driver
                .codex_terminal_input(pid, Some("draft\nsecond"))
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            driver.new_codex_session(pid).await.unwrap(),
            agentix_multiplexer::CodexNewSessionOutcome::CheckoutChoice
        );
        driver
            .select_codex_checkout(
                pid,
                agentix_multiplexer::CodexCheckoutChoice::CurrentCheckout,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(output.with_extension("choice")).unwrap(),
            "Current checkout"
        );
        driver.exit_codex_session(pid).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while std::fs::read_to_string(&output).unwrap_or_default() != "/exit" {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    #[ignore = "creates an isolated session on the local Rex service"]
    async fn local_rex_creation_snapshot_and_split() {
        let directory = tempfile::tempdir().unwrap();
        let driver = RexDriver::default();
        let name = format!("agentix-test-{}", std::process::id());
        let prepared = PreparedMutation {
            mutation: agentix_domain::MultiplexerMutation {
                target: MultiplexerTarget::NewSession {
                    name,
                    cwd: directory.path().to_string_lossy().into_owned(),
                },
                launch_agent: false,
            },
            cwd: directory.path().into(),
        };
        let outcome = driver.execute(&prepared, None).await.unwrap();
        let panes = driver.panes().await.unwrap();
        let pane = panes
            .iter()
            .find(|p| p.pane_id == outcome.location.pane_id)
            .unwrap();
        let connection = driver.connection().await.unwrap();
        let _cleanup = SessionCleanup {
            command: connection.command.clone(),
            endpoint: connection.endpoint.clone(),
            id: pane.session_id.clone(),
        };
        let pid = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(pid) = driver
                    .panes()
                    .await
                    .unwrap()
                    .iter()
                    .find(|p| p.pane_id == pane.pane_id)
                    .and_then(|p| p.foreground_pid)
                {
                    break pid;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("Rex shell did not start within ten seconds");
        let terminal = driver.terminal(pid).await.unwrap();
        let snapshot = terminal.snapshot(pid).await.unwrap();
        assert!(snapshot.cursor_x < 4096);
        let split = PreparedMutation {
            mutation: agentix_domain::MultiplexerMutation {
                target: MultiplexerTarget::SplitPane {
                    pane_id: pane.pane_id.clone(),
                    direction: PaneSplitDirection::Horizontal,
                    cwd: directory.path().to_string_lossy().into_owned(),
                },
                launch_agent: false,
            },
            cwd: directory.path().into(),
        };
        let other = driver.execute(&split, None).await.unwrap();
        assert_ne!(other.location.pane_id, pane.pane_id);
        assert!(
            driver
                .panes()
                .await
                .unwrap()
                .iter()
                .any(|p| p.pane_id == pane.pane_id)
        );
    }
}

#[cfg(test)]
mod pagination_tests {
    use super::*;
    #[test]
    fn blob_chunks_use_data_and_preserve_utf8_byte_offsets() {
        assert_eq!(
            blob_data(&json!({"id":7,"data":"中文","eof":true,"total":6}))
                .unwrap()
                .len(),
            6
        );
        assert!(blob_data(&json!({"content":"wrong field"})).is_err());
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;
    #[test]
    fn ancestry_mapping_rejects_cycles_and_unrelated_processes() {
        let roots = std::collections::HashMap::from([(100u32, "block:one".to_string())]);
        let result = descendants(&roots, "100 1\n200 100\n300 200\n400 500\n500 400\n600 1\n");
        assert_eq!(result.get(&300).map(String::as_str), Some("block:one"));
        assert!(!result.contains_key(&400));
        assert!(!result.contains_key(&600));
    }
}

fn blob_data(value: &Value) -> Result<&str, MultiplexerError> {
    field(value, "data")
}
fn descendants<T: Clone>(
    roots: &std::collections::HashMap<u32, T>,
    text: &str,
) -> std::collections::HashMap<u32, T> {
    let parents: std::collections::HashMap<u32, u32> = text
        .lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            Some((f.next()?.parse().ok()?, f.next()?.parse().ok()?))
        })
        .collect();
    let mut result = std::collections::HashMap::new();
    for &pid in parents.keys() {
        let mut current = pid;
        let mut seen = std::collections::HashSet::new();
        while current != 0 && seen.insert(current) {
            if let Some(location) = roots.get(&current) {
                result.insert(pid, location.clone());
                break;
            }
            current = parents.get(&current).copied().unwrap_or_default();
        }
    }
    result
}
