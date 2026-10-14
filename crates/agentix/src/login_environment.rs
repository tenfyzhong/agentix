//! Initialize the service once, before configuration and backend startup.
use std::ffi::OsString;
use std::os::unix::{ffi::OsStringExt, process::CommandExt};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use nix::unistd::{Uid, User};
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const MARKER: &[u8] = b"\0agentix-service-environment\0";
const ENVIRONMENT_COMMAND: &str =
    r"/usr/bin/printf '\0agentix-service-environment\0'; exec /usr/bin/env -0";

pub(crate) async fn reexec() -> Result<()> {
    if agentix_multiplexer::service_environment_loaded() {
        return Ok(());
    }
    let shell = account_shell().await?;
    let mut delay = Duration::from_secs(1);
    let environment = loop {
        match tokio::time::timeout(TIMEOUT, read(&shell)).await {
            Ok(Ok(environment)) => break environment,
            Ok(Err(error)) => eprintln!(
                "agentix serve: login environment unavailable: {error:#}; retrying in {}s",
                delay.as_secs()
            ),
            Err(_) => eprintln!(
                "agentix serve: login shell environment lookup timed out after 3s; retrying in {}s",
                delay.as_secs()
            ),
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(MAX_RETRY_DELAY);
    };
    // exec preserves the service PID and avoids changing a multithreaded
    // process's global environment. A new child service has a different PID
    // and must load its own snapshot, even if it inherits this marker.
    let error = std::process::Command::new(std::env::current_exe()?)
        .args(std::env::args_os().skip(1))
        .env_clear()
        .envs(environment)
        .env(
            agentix_multiplexer::SERVICE_ENVIRONMENT_PID,
            std::process::id().to_string(),
        )
        .exec();
    Err(error).context("could not restart Agentix serve with login environment")
}

async fn account_shell() -> Result<PathBuf> {
    if let Some(shell) = std::env::var_os("AGENTIX_LOGIN_SHELL") {
        return Ok(PathBuf::from(shell));
    }
    let lookup = || {
        tokio::task::spawn_blocking(|| {
            User::from_uid(Uid::effective())?
                .map(|user| user.shell)
                .context("current user has no account entry")
        })
    };
    let mut task = lookup();
    let mut delay = Duration::from_secs(1);
    loop {
        match tokio::time::timeout(TIMEOUT, &mut task).await {
            Ok(Ok(Ok(shell))) => return Ok(shell),
            Ok(_) => {
                eprintln!(
                    "agentix serve: account shell lookup failed; retrying in {}s",
                    delay.as_secs()
                );
                tokio::time::sleep(delay).await;
                task = lookup();
            }
            Err(_) => {
                // A timed-out system account query may still be running. Reuse
                // its handle rather than accumulating blocking worker threads.
                eprintln!(
                    "agentix serve: account shell lookup timed out after 3s; waiting {}s before checking again",
                    delay.as_secs()
                );
                tokio::time::sleep(delay).await;
            }
        }
        delay = (delay * 2).min(MAX_RETRY_DELAY);
    }
}

async fn read(shell: &std::path::Path) -> Result<Vec<(OsString, OsString)>> {
    ensure!(shell.is_absolute(), "login shell must be an absolute path");
    let child = Command::new(shell)
        .args(["-lc", ENVIRONMENT_COMMAND])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .context("could not run login shell")?;
    let _group = LoginProcessGroup(nix::unistd::Pid::from_raw(i32::try_from(
        child.id().context("login shell has no process ID")?,
    )?));
    let output = child
        .wait_with_output()
        .await
        .context("could not read login shell output")?;
    ensure!(output.status.success(), "login shell exited unsuccessfully");
    parse(&output.stdout)
}

// Cancellation drops this guard as well as the kill-on-drop child. Kill the
// entire group so startup helpers cannot survive a timeout or service shutdown.
// Tokio reaps the direct child when its wait future is cancelled.
struct LoginProcessGroup(nix::unistd::Pid);

impl Drop for LoginProcessGroup {
    fn drop(&mut self) {
        let _ = nix::sys::signal::killpg(self.0, nix::sys::signal::Signal::SIGKILL);
    }
}

fn parse(bytes: &[u8]) -> Result<Vec<(OsString, OsString)>> {
    let start = bytes
        .windows(MARKER.len())
        .position(|part| part == MARKER)
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
    fn snapshot_accepts_no_exported_variables() {
        assert!(parse(MARKER).unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelled_lookup_terminates_shell_descendants() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let shell = directory.path().join("shell");
        let helper = directory.path().join("helper");
        let pid_file = directory.path().join("pid");
        let escaped = directory.path().join("escaped");
        std::fs::write(
            &helper,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nsleep 2\ntouch '{}'\n",
                pid_file.display(),
                escaped.display()
            ),
        )
        .unwrap();
        std::fs::write(&shell, format!("#!/bin/sh\n'{}'\n", helper.display())).unwrap();
        for path in [&shell, &helper] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        {
            let lookup = read(&shell);
            tokio::pin!(lookup);
            tokio::select! {
                result = &mut lookup => panic!("lookup finished before cancellation: {result:?}"),
                () = async {
                    tokio::time::timeout(Duration::from_secs(5), async {
                        while !pid_file.exists() {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }).await.unwrap();
                } => {}
            }
            // Timeout drops the same future as service shutdown cancellation.
            assert!(
                tokio::time::timeout(Duration::from_millis(20), lookup)
                    .await
                    .is_err()
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !escaped.exists(),
            "cancelled shell left a running descendant"
        );
    }

    #[test]
    fn snapshot_preserves_bytes_empty_values_and_startup_noise() {
        let values = parse(b"banner\n\0agentix-service-environment\0VALUE=with\nnewline=and spaces \0EMPTY=\0BYTES=\xff\0").unwrap();
        assert_eq!(
            values,
            vec![
                (
                    OsString::from("VALUE"),
                    OsString::from("with\nnewline=and spaces ")
                ),
                (OsString::from("EMPTY"), OsString::new()),
                (OsString::from("BYTES"), OsString::from_vec(vec![0xff])),
            ]
        );
    }

    #[test]
    fn invalid_snapshots_do_not_expose_values_in_errors() {
        for bytes in [
            b"secret".as_slice(),
            b"\0agentix-service-environment\0KEY=secret",
            b"\0agentix-service-environment\0secret\0",
            b"\0agentix-service-environment\0=secret\0",
        ] {
            assert!(!parse(bytes).unwrap_err().to_string().contains("secret"));
        }
    }
}
