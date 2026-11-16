use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::Read,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    time::{Duration, Instant},
};

pub(crate) fn validate_entry(entry: &str) -> Result<()> {
    ensure!(
        !entry.trim().is_empty()
            && !entry.starts_with(['-', '/'])
            && !entry.split('/').any(|part| part == "..")
            && !entry.chars().any(char::is_control),
        "invalid pass entry"
    );
    Ok(())
}

// Cache successful decryptions only; environment identity keeps separate stores isolated.
pub(crate) fn key(entry: &str) -> Result<String> {
    type Cache = HashMap<Vec<OsString>, String>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    validate_entry(entry)?;
    let identity = std::iter::once(OsString::from(entry))
        .chain(
            ["HOME", "GNUPGHOME", "PASSWORD_STORE_DIR", "PATH"]
                .map(|name| std::env::var_os(name).unwrap_or_default()),
        )
        .collect::<Vec<_>>();
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| anyhow::anyhow!("pass cache unavailable"))?;
    if let Some(key) = cache.get(&identity) {
        return Ok(key.clone());
    }
    let mut command = Command::new("pass");
    command.args(["show", "--", entry]);
    let key = read_key(command, Duration::from_mins(2))?;
    cache.insert(identity, key.clone());
    Ok(key)
}

fn read_key(mut command: Command, timeout: Duration) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            anyhow::anyhow!(
                "could not run pass; install pass and configure pinentry or use api_key_env"
            )
        })?;
    let stdout = child.stdout.take().context("pass stdout unavailable")?;
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(65537).read_to_end(&mut bytes).map(|_| bytes);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let bytes = rx
            .recv_timeout(timeout)
            .map_err(|_| {
                anyhow::anyhow!("pass decryption timed out; configure pinentry or use api_key_env")
            })?
            .map_err(|_| anyhow::anyhow!("could not read pass output"))?;
        ensure!(bytes.len() <= 65536, "pass output exceeds limit");
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|_| anyhow::anyhow!("could not wait for pass"))?
            {
                ensure!(
                    status.success(),
                    "pass decryption failed; configure pinentry or use api_key_env"
                );
                break;
            }
            if Instant::now() >= deadline {
                bail!("pass decryption timed out; configure pinentry or use api_key_env");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let first = bytes.split(|b| *b == b'\n').next().unwrap_or_default();
        let first = first.strip_suffix(b"\r").unwrap_or(first);
        ensure!(
            !first.is_empty() && first.iter().all(|b| (0x20..=0x7e).contains(b)),
            "pass returned an invalid API key"
        );
        let key = String::from_utf8(first.to_vec()).context("pass returned an invalid API key")?;
        ensure!(!key.trim().is_empty(), "pass returned an empty API key");
        Ok(key)
    })();
    // Stop the whole command group on timeout, including GPG children holding stdout.
    #[cfg(unix)]
    if let Some(pid) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    if reader.is_finished() {
        let _ = reader.join();
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn first_line_only_and_timeout_are_bounded() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'test-key\\r\\nmetadata\\n'"]);
        assert_eq!(
            read_key(command, Duration::from_secs(1)).unwrap(),
            "test-key"
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 10"]);
        let start = Instant::now();
        assert!(
            read_key(command, Duration::from_millis(30))
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
