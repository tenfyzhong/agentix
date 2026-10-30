//! Backend-neutral terminal reads and guarded input.
use crate::{MultiplexerError, native_control::run};
use async_trait::async_trait;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    pub command: String,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub in_mode: bool,
    pub dead: bool,
    pub screen: String,
    pub plain_screen: String,
}
impl TerminalSnapshot {
    pub(crate) fn codex_state(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.command,
            self.cursor_x,
            self.cursor_y,
            u8::from(self.in_mode),
            u8::from(self.dead)
        )
    }
}

/// Implementations must revalidate the original process before every operation.
#[async_trait]
pub trait TerminalControl: Send + Sync {
    async fn snapshot(&self, pid: u32) -> Result<TerminalSnapshot, MultiplexerError>;
    async fn send_key(&self, pid: u32, key: &str) -> Result<(), MultiplexerError>;
    async fn send_text(&self, pid: u32, text: &str) -> Result<(), MultiplexerError>;
}

pub(crate) struct TmuxTerminal<'a> {
    pub command: &'a Path,
    pub prefix: &'a [String],
    pub pane: &'a str,
    pub allowed_processes: &'static [&'static str],
}

pub async fn verify_foreground(pid: u32) -> Result<String, MultiplexerError> {
    let process = run(
        Path::new("ps"),
        &[],
        &["-p", &pid.to_string(), "-o", "pgid=,tpgid=,comm="],
    )
    .await?;
    let fields: Vec<_> = process.split_whitespace().collect();
    if fields.len() < 3 || fields[0] != fields[1] || ["0", "-1"].contains(&fields[0]) {
        return Err(MultiplexerError::Backend(
            "Original agent process is no longer in the foreground".into(),
        ));
    }
    Ok(crate::command_basename(fields[2]))
}

impl TmuxTerminal<'_> {
    async fn verify(&self, pid: u32) -> Result<(), MultiplexerError> {
        let name = verify_foreground(pid).await?;
        if !self.allowed_processes.contains(&name.as_str()) {
            return Err(MultiplexerError::Backend(
                "Original agent process changed".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl TerminalControl for TmuxTerminal<'_> {
    async fn snapshot(&self, pid: u32) -> Result<TerminalSnapshot, MultiplexerError> {
        self.verify(pid).await?;
        let state = run(
            self.command,
            self.prefix,
            &[
                "display-message",
                "-p",
                "-t",
                self.pane,
                "#{pane_current_command}|#{cursor_x}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}",
            ],
        )
        .await?;
        let fields: Vec<_> = state.trim().split('|').collect();
        let invalid = || MultiplexerError::Backend("Invalid terminal state".into());
        if fields.len() != 5 || !["0", "1"].contains(&fields[3]) || !["0", "1"].contains(&fields[4])
        {
            return Err(invalid());
        }
        let screen = run(
            self.command,
            self.prefix,
            &["capture-pane", "-p", "-e", "-t", self.pane],
        )
        .await?;
        let plain_screen = run(
            self.command,
            self.prefix,
            &["capture-pane", "-p", "-t", self.pane],
        )
        .await?;
        Ok(TerminalSnapshot {
            command: fields[0].into(),
            cursor_x: fields[1].parse().map_err(|_| invalid())?,
            cursor_y: fields[2].parse().map_err(|_| invalid())?,
            in_mode: fields[3] != "0",
            dead: fields[4] != "0",
            screen,
            plain_screen,
        })
    }
    async fn send_key(&self, pid: u32, key: &str) -> Result<(), MultiplexerError> {
        self.verify(pid).await?;
        run(
            self.command,
            self.prefix,
            &["send-keys", "-t", self.pane, key],
        )
        .await?;
        Ok(())
    }
    async fn send_text(&self, pid: u32, text: &str) -> Result<(), MultiplexerError> {
        self.verify(pid).await?;
        run(
            self.command,
            self.prefix,
            &["send-keys", "-t", self.pane, "-l", text],
        )
        .await?;
        Ok(())
    }
}
