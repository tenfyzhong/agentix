use std::ffi::OsString;
use std::os::unix::{ffi::OsStringExt, process::CommandExt};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use nix::unistd::{Uid, User};
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(3);
const MARKER: &[u8] = b"\0taskix-login-environment\0";
const LOADED_PID: &str = "TASKIX_MEMORY_LOGIN_ENVIRONMENT_PID";
const ENVIRONMENT_COMMAND: &str =
    r"/usr/bin/printf '\0taskix-login-environment\0'; exec /usr/bin/env -0";

pub(super) async fn reexec() -> Result<()> {
    let pid = std::process::id().to_string();
    if std::env::var(LOADED_PID).as_deref() == Ok(pid.as_str()) {
        return Ok(());
    }
    let environment = match tokio::time::timeout(TIMEOUT, read()).await {
        Ok(Ok(environment)) => environment,
        Ok(Err(error)) => {
            eprintln!(
                "taskix memory: could not load login shell environment: {error:#}; using inherited environment"
            );
            return Ok(());
        }
        Err(_) => {
            eprintln!(
                "taskix memory: login shell environment lookup timed out; using inherited environment"
            );
            return Ok(());
        }
    };
    // Replace the service without changing its PID or mutating a multithreaded
    // process's environment. The PID marker prevents recursive shell lookups
    // while allowing separately spawned services to load their own snapshot.
    let error = std::process::Command::new(std::env::current_exe()?)
        .args(std::env::args_os().skip(1))
        .env_clear()
        .envs(environment)
        .env(LOADED_PID, pid)
        .exec();
    Err(error).context("could not restart memory service with login environment")
}

async fn read() -> Result<Vec<(OsString, OsString)>> {
    let shell = match std::env::var_os("TASKIX_LOGIN_SHELL") {
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
        .context("could not run login shell")?;
    ensure!(output.status.success(), "login shell exited unsuccessfully");
    parse(&output.stdout)
}

fn parse(bytes: &[u8]) -> Result<Vec<(OsString, OsString)>> {
    let start = bytes
        .windows(MARKER.len())
        .position(|bytes| bytes == MARKER)
        .context("login shell did not return an environment marker")?
        + MARKER.len();
    let bytes = &bytes[start..];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_framing_preserves_bytes_and_skips_startup_noise() {
        let environment = parse(b"startup message\n\0taskix-login-environment\0KEY= spaces\nand=equals \0EMPTY=\0BYTES=\xff\0").unwrap();
        assert_eq!(
            environment,
            vec![
                (
                    OsString::from("KEY"),
                    OsString::from(" spaces\nand=equals ")
                ),
                (OsString::from("EMPTY"), OsString::new()),
                (OsString::from("BYTES"), OsString::from_vec(vec![0xff])),
            ]
        );
        assert!(parse(MARKER).unwrap().is_empty());
    }

    #[test]
    fn invalid_environment_framing_is_rejected_without_values_in_errors() {
        for bytes in [
            b"secret-value".as_slice(),
            b"\0taskix-login-environment\0KEY=secret-value",
            b"\0taskix-login-environment\0secret-value\0",
            b"\0taskix-login-environment\0=secret-value\0",
        ] {
            let error = parse(bytes).unwrap_err().to_string();
            assert!(!error.contains("secret-value"));
        }
    }
}
