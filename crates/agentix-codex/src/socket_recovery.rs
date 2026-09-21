//! Recover only an abandoned socket or a positively identified Codex frontend owner.
use std::io::{self, ErrorKind};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::time::Duration;
use tokio::net::UnixStream;

fn occupied(message: &str) -> io::Error {
    io::Error::new(ErrorKind::AddrInUse, message)
}

fn identity(path: &Path) -> io::Result<(u64, u64)> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_socket() || meta.uid() != nix::unistd::geteuid().as_raw() {
        return Err(occupied("existing path is not a socket owned by this user"));
    }
    Ok((meta.dev(), meta.ino()))
}

async fn connect(path: &Path) -> io::Result<UnixStream> {
    tokio::time::timeout(Duration::from_secs(1), UnixStream::connect(path))
        .await
        .map_err(|_| occupied("socket probe timed out; owner cannot be established"))?
}

pub(crate) async fn recover(path: &Path) -> io::Result<()> {
    let original = match identity(path) {
        Ok(value) => value,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    match connect(path).await {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {}
        Err(error) => return Err(error),
        Ok(stream) => {
            let pid = crate::proxy::unix_pid(&stream)
                .filter(|pid| *pid != std::process::id())
                .ok_or_else(|| occupied("socket belongs to an unknown or current process"))?;
            if stream.peer_cred()?.uid() != nix::unistd::geteuid().as_raw()
                || !is_codex_app_server(pid).await?
            {
                return Err(occupied(
                    "socket listener is not a verified Codex app-server",
                ));
            }
            drop(stream);
            for signal in [
                nix::sys::signal::Signal::SIGTERM,
                nix::sys::signal::Signal::SIGKILL,
            ] {
                // Recheck the inode, peer and executable immediately before each signal.
                if identity(path)? != original {
                    return Err(occupied("socket owner changed during recovery"));
                }
                match connect(path).await {
                    Ok(stream) if crate::proxy::unix_pid(&stream) == Some(pid) => {}
                    Err(error) if error.kind() == ErrorKind::ConnectionRefused => break,
                    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                    _ => return Err(occupied("socket peer changed during recovery")),
                }
                if !is_codex_app_server(pid).await? {
                    return Err(occupied("socket process changed during recovery"));
                }
                tracing::warn!(pid, ?signal, path = %path.display(), "reclaiming Codex proxy socket from app-server");
                match nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(i32::try_from(pid).map_err(io::Error::other)?),
                    signal,
                ) {
                    Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                    Err(error) => return Err(io::Error::from_raw_os_error(error as i32)),
                }
                let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                loop {
                    match connect(path).await {
                        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                        Err(error) if error.kind() == ErrorKind::ConnectionRefused => break,
                        Ok(stream) if crate::proxy::unix_pid(&stream) == Some(pid) => {}
                        _ => return Err(occupied("socket peer changed while stopping app-server")),
                    }
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
    }
    // Never unlink a replacement or a listener that became available during inspection.
    if identity(path)? != original {
        return Err(occupied("socket was replaced during recovery"));
    }
    match connect(path).await {
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
            if identity(path)? != original {
                return Err(occupied("socket was replaced during recovery"));
            }
            std::fs::remove_file(path)?;
            tracing::info!(path = %path.display(), "removed abandoned Codex socket");
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        _ => Err(occupied("socket still has a listener after recovery")),
    }
}

async fn ps_field(pid: u32, field: &str) -> io::Result<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(1),
        tokio::process::Command::new("ps")
            .args(["-ww", "-p", &pid.to_string(), "-o", field])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| occupied("process inspection timed out"))??;
    if !output.status.success() {
        return Err(occupied("socket process could not be inspected"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

async fn is_codex_app_server(pid: u32) -> io::Result<bool> {
    #[cfg(target_os = "macos")]
    let executable = ps_field(pid, "ucomm=").await?;
    #[cfg(not(target_os = "macos"))]
    let executable = ps_field(pid, "comm=").await?;
    let args = ps_field(pid, "args=").await?;
    Ok(codex_app_server_command(&executable, &args))
}

fn codex_app_server_command(executable: &str, args: &str) -> bool {
    if Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        != Some("codex")
    {
        return false;
    }
    let arguments = args
        .strip_prefix(executable)
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .or_else(|| {
            let (program, rest) = args.split_once(char::is_whitespace)?;
            (Path::new(program).file_name()?.to_str()? == "codex").then_some(rest)
        });
    arguments.is_some_and(|rest| rest.split_whitespace().next() == Some("app-server"))
}

#[cfg(test)]
mod tests {
    use super::codex_app_server_command;

    #[test]
    fn identifies_app_server_without_matching_client_arguments_or_other_executables() {
        for (executable, args, expected) in [
            (
                "codex",
                "codex app-server --listen unix:///tmp/a.sock",
                true,
            ),
            ("/opt/bin/codex", "/opt/bin/codex app-server", true),
            ("codex", "/opt/bin/codex app-server", true),
            (
                "/opt/my tools/codex",
                "/opt/my tools/codex app-server",
                true,
            ),
            ("agentix", "codex app-server", false),
            ("codex", "codex exec app-server", false),
            ("codex", "codex --remote unix:///tmp/a.sock", false),
            ("codex", "codex app-server-proxy", false),
            ("codex", "codex", false),
            ("codex", "sh -c codex app-server", false),
        ] {
            assert_eq!(
                codex_app_server_command(executable, args),
                expected,
                "{executable}: {args}"
            );
        }
    }
}
