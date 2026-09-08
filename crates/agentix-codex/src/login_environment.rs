use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use nix::unistd::{Uid, User};
use tokio::process::Command;

const LOGIN_ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(3);
const ENVIRONMENT_MARKER: &[u8] = b"\0agentix-login-environment\0";
// NUL framing separates startup messages and preserves arbitrary environment
// values. Replacing the shell prevents exit hooks from appending stdout noise.
const ENVIRONMENT_COMMAND: &str =
    r"/usr/bin/printf '\0agentix-login-environment\0'; exec /usr/bin/env -0";

pub(crate) async fn apply(command: &mut Command) {
    match tokio::time::timeout(LOGIN_ENVIRONMENT_TIMEOUT, read()).await {
        Ok(Ok(environment)) => {
            // Use the complete snapshot so shell `unset` operations also apply.
            // Never mutate Agentix's own process environment.
            command.env_clear().envs(environment);
        }
        Ok(Err(error)) => {
            tracing::warn!(%error, "Could not load login shell environment; using inherited environment for Codex daemon");
        }
        Err(_) => {
            tracing::warn!(
                "Login shell environment lookup timed out; using inherited environment for Codex daemon"
            );
        }
    }
}

async fn read() -> Result<Vec<(OsString, OsString)>> {
    let shell = match std::env::var_os("AGENTIX_LOGIN_SHELL") {
        Some(shell) => PathBuf::from(shell),
        None => {
            tokio::task::spawn_blocking(|| {
                User::from_uid(Uid::effective())?
                    .map(|user| user.shell)
                    .context("current user has no account entry")
            })
            .await??
        }
    };
    ensure!(shell.is_absolute(), "login shell must be an absolute path");
    let output = Command::new(&shell)
        .args(["-lc", ENVIRONMENT_COMMAND])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("could not run login shell {}", shell.display()))?;
    ensure!(
        output.status.success(),
        "login shell exited with {}",
        output.status
    );
    let start = output
        .stdout
        .windows(ENVIRONMENT_MARKER.len())
        .position(|bytes| bytes == ENVIRONMENT_MARKER)
        .context("login shell did not return an environment marker")?
        + ENVIRONMENT_MARKER.len();
    let bytes = &output.stdout[start..];
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    ensure!(
        bytes.ends_with(&[0]),
        "login shell returned an incomplete environment"
    );
    bytes[..bytes.len() - 1]
        .split(|byte| *byte == 0)
        .map(|entry| {
            let separator = entry
                .iter()
                .position(|byte| *byte == b'=')
                .context("login shell returned a malformed environment entry")?;
            ensure!(
                separator > 0,
                "login shell returned an empty environment key"
            );
            Ok((
                OsString::from_vec(entry[..separator].to_vec()),
                OsString::from_vec(entry[separator + 1..].to_vec()),
            ))
        })
        .collect()
}
