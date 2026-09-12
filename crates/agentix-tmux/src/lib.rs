//! tmux CLI adapter with explicit server selection and bounded child processes.
use agentix_domain::{MultiplexerKind, MultiplexerTarget, PaneSplitDirection, TerminalLocation};
use agentix_multiplexer::{
    MultiplexerDriver, MultiplexerError, MultiplexerOutcome, PaneState, PreparedMutation,
    is_shell_command, terminal_location,
};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(5);
const FORMAT: &str = "#{session_id}|#{session_name}|#{window_id}|#{window_index}|#{window_name}|#{pane_id}|#{pane_index}|#{pane_active}|#{pane_current_command}|#{pane_current_path}|#{pane_pid}|#{pane_tty}";

#[derive(Debug)]
pub struct TmuxDriver {
    command: PathBuf,
    socket: Option<PathBuf>,
}
impl Default for TmuxDriver {
    fn default() -> Self {
        Self::with_command("tmux".into(), None)
    }
}
impl TmuxDriver {
    #[must_use]
    pub fn with_command(command: PathBuf, socket: Option<PathBuf>) -> Self {
        Self { command, socket }
    }
    async fn output(&self, args: &[String]) -> Result<Output, MultiplexerError> {
        if cfg!(windows) {
            return Err(error("tmux control is unavailable on Windows"));
        }
        let mut command = Command::new(&self.command);
        if let Some(socket) = &self.socket {
            command.arg("-S").arg(socket);
        } else {
            command.args(["-L", "default"]);
        }
        command
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env("LC_ALL", "C");
        command.args(args).stdin(Stdio::null()).kill_on_drop(true);
        tokio::time::timeout(TIMEOUT, command.output())
            .await
            .map_err(|_| error("tmux operation timed out"))?
            .map_err(error)
    }
    async fn run(&self, args: &[String]) -> Result<String, MultiplexerError> {
        let output = self.output(args).await?;
        if !output.status.success() {
            return Err(error(String::from_utf8_lossy(&output.stderr)));
        }
        String::from_utf8(output.stdout).map_err(error)
    }
    async fn launch(&self, pane: &str, cwd: &str, argv: &[String]) -> Result<(), MultiplexerError> {
        self.run(&strings(&[
            "set-option",
            "-p",
            "-t",
            pane,
            "remain-on-exit",
            "off",
        ]))
        .await?;
        let mut command_args = strings(&[
            "respawn-pane",
            "-k",
            "-t",
            pane,
            "-c",
            cwd,
            "--",
            "/usr/bin/env",
            "--",
            // Interactive mode preserves foreground job control and Ctrl-C.
            "/bin/sh",
            "-i",
            "-c",
            r#""$@"; exec "${SHELL:-/bin/sh}" -i"#,
            "agentix",
        ]);
        command_args.extend(argv.iter().map(|value| {
            // tmux parses command separators even with structured argv.
            value
                .strip_suffix(';')
                .map_or_else(|| value.clone(), |prefix| format!("{prefix}\\;"))
        }));
        self.run(&command_args).await?;
        Ok(())
    }
    async fn location(&self, id: &str) -> Result<TerminalLocation, MultiplexerError> {
        self.inventory(false)
            .await?
            .unwrap_or_default()
            .iter()
            .find(|pane| pane.pane_id == id)
            .map(terminal_location)
            .ok_or_else(|| error("tmux pane disappeared after creation"))
    }
}

#[async_trait]
impl MultiplexerDriver for TmuxDriver {
    fn kind(&self) -> MultiplexerKind {
        MultiplexerKind::Tmux
    }

    async fn inventory(&self, _start: bool) -> Result<Option<Vec<PaneState>>, MultiplexerError> {
        let output = self
            .output(&strings(&["list-panes", "-a", "-F", FORMAT]))
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no server running")
                || (stderr.contains("error connecting to")
                    && stderr.contains("No such file or directory"))
            {
                return Ok(Some(Vec::new()));
            }
            return Err(error(stderr));
        }
        parse_inventory(&String::from_utf8(output.stdout).map_err(error)?).map(Some)
    }

    async fn execute(
        &self,
        prepared: &PreparedMutation,
        argv: Option<&[String]>,
    ) -> Result<MultiplexerOutcome, MultiplexerError> {
        let cwd = prepared.cwd.to_string_lossy();
        let pane = match &prepared.mutation.target {
            MultiplexerTarget::NewSession { name, .. } => {
                self.run(&strings(&[
                    "new-session",
                    "-d",
                    "-s",
                    name,
                    "-n",
                    name,
                    "-c",
                    &cwd,
                    "-P",
                    "-F",
                    "#{pane_id}",
                ]))
                .await?
            }
            MultiplexerTarget::NewWindow {
                session_id, name, ..
            } => {
                self.run(&strings(&[
                    "new-window",
                    "-d",
                    "-t",
                    session_id,
                    "-n",
                    name,
                    "-c",
                    &cwd,
                    "-P",
                    "-F",
                    "#{pane_id}",
                ]))
                .await?
            }
            MultiplexerTarget::SplitPane {
                pane_id, direction, ..
            } => {
                let flag = match direction {
                    PaneSplitDirection::Horizontal => "-h",
                    PaneSplitDirection::Vertical => "-v",
                };
                self.run(&strings(&[
                    "split-window",
                    "-d",
                    flag,
                    "-t",
                    pane_id,
                    "-c",
                    &cwd,
                    "-P",
                    "-F",
                    "#{pane_id}",
                ]))
                .await?
            }
            MultiplexerTarget::ExistingPane { pane_id } => {
                if argv.is_none() {
                    return Err(error("an existing pane must launch an agent"));
                }
                let inventory = self.inventory(false).await?.unwrap_or_default();
                let pane = inventory
                    .iter()
                    .find(|p| &p.pane_id == pane_id)
                    .ok_or_else(|| error("tmux pane no longer exists"))?;
                if !is_shell_command(&pane.current_command) {
                    return Err(MultiplexerError::BusyPane {
                        pane_id: pane_id.clone(),
                        command: pane.current_command.clone(),
                    });
                }
                self.run(&strings(&["send-keys", "-t", pane_id, "C-c"]))
                    .await?;
                pane_id.clone()
            }
        };
        let pane = pane.trim();
        if let Some(argv) = argv {
            self.launch(pane, &cwd, argv).await?;
        }
        Ok(MultiplexerOutcome {
            location: self.location(pane).await?,
        })
    }

    async fn process_locations(&self) -> Result<HashMap<u32, TerminalLocation>, MultiplexerError> {
        let panes = self.inventory(false).await?.unwrap_or_default();
        if panes.is_empty() {
            return Ok(HashMap::new());
        }
        let mut command = Command::new("ps");
        command.args(["-A", "-o", "pid=,ppid="]).kill_on_drop(true);
        let output = tokio::time::timeout(TIMEOUT, command.output())
            .await
            .map_err(error)?
            .map_err(error)?;
        if !output.status.success() {
            return Err(error(String::from_utf8_lossy(&output.stderr)));
        }
        Ok(descendant_locations(
            &panes,
            &String::from_utf8_lossy(&output.stdout),
        ))
    }
}

fn error(value: impl std::fmt::Display) -> MultiplexerError {
    MultiplexerError::Backend(value.to_string())
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

fn parse_inventory(text: &str) -> Result<Vec<PaneState>, MultiplexerError> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split('|').collect();
            let invalid = || error("invalid tmux inventory record");
            if fields.len() != 12
                || !fields[0].starts_with('$')
                || !fields[2].starts_with('@')
                || !fields[5].starts_with('%')
            {
                return Err(invalid());
            }
            Ok(PaneState {
                multiplexer: MultiplexerKind::Tmux,
                session_id: fields[0].into(),
                session_name: fields[1].into(),
                window_id: fields[2].into(),
                window_index: fields[3].parse().map_err(|_| invalid())?,
                window_name: fields[4].into(),
                pane_id: fields[5].into(),
                pane_index: fields[6].parse().map_err(|_| invalid())?,
                active: match fields[7] {
                    "0" => false,
                    "1" => true,
                    _ => return Err(invalid()),
                },
                current_command: fields[8].into(),
                cwd: fields[9].into(),
                foreground_pid: Some(fields[10].parse().map_err(|_| invalid())?),
            })
        })
        .collect()
}

fn descendant_locations(panes: &[PaneState], processes: &str) -> HashMap<u32, TerminalLocation> {
    let roots: HashMap<_, _> = panes
        .iter()
        .filter_map(|p| p.foreground_pid.map(|pid| (pid, terminal_location(p))))
        .collect();
    let parents: HashMap<u32, u32> = processes
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
        })
        .collect();
    let mut result = HashMap::new();
    for &pid in parents.keys() {
        let mut current = pid;
        let mut seen = HashSet::new();
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

#[cfg(test)]
mod tests;
