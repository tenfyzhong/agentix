//! All terminal subprocesses use the login shell PATH before execution.
use super::MultiplexerError;
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

/// Execute once with the login PATH, preserving all other command settings.
/// The deadline includes both login environment lookup and command execution.
pub async fn terminal_command_output(command: &mut Command) -> Result<Output, MultiplexerError> {
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    tokio::time::timeout(Duration::from_secs(5), async {
        #[cfg(unix)]
        {
            let path = login_path().await.ok_or_else(|| {
                MultiplexerError::Backend(format!("could not read login shell PATH for {program}"))
            })?;
            command.env("PATH", path);
        }
        command
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|failure| {
                MultiplexerError::Backend(format!("could not start {program}: {failure}"))
            })
    })
    .await
    .map_err(|failure| MultiplexerError::Backend(format!("{program}: {failure}")))?
}

#[cfg(unix)]
async fn login_path() -> Option<std::ffi::OsString> {
    use std::{ffi::OsString, path::PathBuf, time::Instant};
    use tokio::sync::Mutex;

    struct CachedPath {
        shell: PathBuf,
        path: OsString,
        read_at: Instant,
    }
    static PATH: Mutex<Option<CachedPath>> = Mutex::const_new(None);

    let shell = PathBuf::from(super::launch::login_shell());
    // Polling and input validation issue several commands per pane. Resolve
    // their login PATH once, including concurrent requests, then refresh it.
    let mut cached = PATH.lock().await;
    if let Some(entry) = cached.as_ref()
        && entry.shell == shell
        && entry.read_at.elapsed() < Duration::from_mins(1)
    {
        return Some(entry.path.clone());
    }
    let path = read_login_path(&shell).await?;
    *cached = Some(CachedPath {
        shell,
        path: path.clone(),
        read_at: Instant::now(),
    });
    Some(path)
}

#[cfg(unix)]
async fn read_login_path(shell: &std::path::Path) -> Option<std::ffi::OsString> {
    use std::os::unix::ffi::OsStringExt;
    use std::process::Stdio;

    const MARKER: &[u8] = b"\0agentix-terminal-path\0";
    // Framing excludes shell startup output; env preserves spaces and newlines.
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        Command::new(shell)
            .args([
                "-lc",
                r"/usr/bin/printf '\0agentix-terminal-path\0'; exec /usr/bin/env -0",
            ])
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() || !output.stdout.ends_with(&[0]) {
        return None;
    }
    let start = output
        .stdout
        .windows(MARKER.len())
        .position(|part| part == MARKER)?
        + MARKER.len();
    output.stdout[start..]
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(b"PATH="))
        .map(|path| std::ffi::OsString::from_vec(path.to_vec()))
}
