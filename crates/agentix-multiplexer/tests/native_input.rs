//! Isolated real terminal transport; no model provider or developer session.
use agentix_multiplexer::{codex_terminal_input, send_codex_new};
use std::{path::Path, process::Command, time::Duration};

fn output(command: &str, args: &[&str]) -> String {
    let result = Command::new(command).args(args).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

struct Cleanup(String, String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new(&self.0)
            .args(["-S", &self.1, "kill-server"])
            .output();
    }
}
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_codex_draft_is_read_and_cleared_only_after_matching_confirmation() {
    if std::env::var_os("AGENTIX_TEST_NATIVE_HOSTS").is_none() {
        return;
    }
    for (driver, style) in [
        ("tmux", "colored"),
        ("rmux", "colored"),
        ("tmux", "plain"),
        ("rmux", "plain"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let node = std::fs::canonicalize(output("which", &["node"])).unwrap();
        let executable = bin.join("codex");
        std::fs::copy(&node, &executable).unwrap();
        #[cfg(unix)]
        {
            let libraries = node.parent().unwrap().join("../lib");
            if libraries.exists() {
                std::os::unix::fs::symlink(libraries, root.path().join("lib")).unwrap();
            }
        }
        let socket = root.path().join("server.sock");
        let socket = socket.to_str().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/agentix-bridge/tests/claude-rmux-host.mjs");
        let command = format!(
            "exec {} {} {} 'local draft\nsecond line' codex {style}",
            quote(&executable),
            quote(&fixture),
            quote(&root.path().join("sent"))
        );
        let pane = output(
            driver,
            &[
                "-S",
                socket,
                "new-session",
                "-d",
                "-s",
                "test",
                "-x",
                "100",
                "-y",
                "30",
                "-P",
                "-F",
                "#{pane_id}",
                &command,
            ],
        );
        let _cleanup = Cleanup(driver.into(), socket.into());
        let prefix = vec!["-S".into(), socket.into()];
        let pid = output(
            driver,
            &[
                "-S",
                socket,
                "display-message",
                "-p",
                "-t",
                &pane,
                "#{pane_pid}",
            ],
        )
        .parse()
        .unwrap();
        let mut draft = None;
        for _ in 0..30 {
            match codex_terminal_input(Path::new(driver), &prefix, &pane, pid, None).await {
                Ok(value) => {
                    draft = value;
                    break;
                }
                Err(error) => {
                    eprintln!("{driver}: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
        assert_eq!(
            draft.as_deref(),
            Some("local draft\nsecond line"),
            "{driver}"
        );
        assert_eq!(
            codex_terminal_input(Path::new(driver), &prefix, &pane, pid, Some("different"))
                .await
                .unwrap(),
            draft
        );
        assert_eq!(
            codex_terminal_input(
                Path::new(driver),
                &prefix,
                &pane,
                pid,
                Some("local draft\nsecond line")
            )
            .await
            .unwrap(),
            None
        );
        assert!(!root.path().join("sent").exists());
        send_codex_new(Path::new(driver), &prefix, &pane, pid)
            .await
            .unwrap();
        for _ in 0..30 {
            if root.path().join("sent").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(
            std::fs::read_to_string(root.path().join("sent")).unwrap(),
            "/new"
        );
    }
}

/// Opt-in real Codex TUI validation against a supplied local app-server.
/// Sends only /new and an unsubmitted local draft; no model prompt is submitted.
#[tokio::test]
async fn real_codex_submits_new_from_empty_and_confirmed_draft() {
    let (Ok(binary), Ok(endpoint)) = (
        std::env::var("AGENTIX_TEST_CODEX_BINARY"),
        std::env::var("AGENTIX_TEST_CODEX_ENDPOINT"),
    ) else {
        return;
    };
    for driver in ["tmux", "rmux"] {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("server.sock");
        let socket = socket.to_str().unwrap();
        let command = format!(
            "exec {} --remote {} --no-alt-screen --dangerously-bypass-approvals-and-sandbox -C {}",
            quote(Path::new(&binary)),
            quote(Path::new(&endpoint)),
            quote(root.path())
        );
        let pane = output(
            driver,
            &[
                "-S",
                socket,
                "new-session",
                "-d",
                "-s",
                "test",
                "-x",
                "120",
                "-y",
                "35",
                "-P",
                "-F",
                "#{pane_id}",
                &command,
            ],
        );
        let _cleanup = Cleanup(driver.into(), socket.into());
        let prefix = vec!["-S".into(), socket.into()];
        let pid = output(
            driver,
            &[
                "-S",
                socket,
                "display-message",
                "-p",
                "-t",
                &pane,
                "#{pane_pid}",
            ],
        )
        .parse()
        .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let startup = output(driver, &["-S", socket, "capture-pane", "-p", "-t", &pane]);
        if startup.contains("Do you trust the contents of this directory?") {
            output(driver, &["-S", socket, "send-keys", "-t", &pane, "Enter"]);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        wait_for_empty_codex(driver, &prefix, &pane, pid).await;
        for draft in [None, Some("local test draft")] {
            if let Some(draft) = draft {
                output(
                    driver,
                    &["-S", socket, "send-keys", "-t", &pane, "-l", draft],
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
                assert_eq!(
                    codex_terminal_input(Path::new(driver), &prefix, &pane, pid, None)
                        .await
                        .unwrap()
                        .as_deref(),
                    Some(draft)
                );
                assert_eq!(
                    codex_terminal_input(Path::new(driver), &prefix, &pane, pid, Some(draft))
                        .await
                        .unwrap(),
                    None
                );
            }
            if let Err(error) = send_codex_new(Path::new(driver), &prefix, &pane, pid).await {
                let state = output(
                    driver,
                    &[
                        "-S",
                        socket,
                        "display-message",
                        "-p",
                        "-t",
                        &pane,
                        "#{pane_current_command}|#{cursor_x}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}",
                    ],
                );
                let screen = output(driver, &["-S", socket, "capture-pane", "-p", "-t", &pane]);
                panic!("{driver}: {error}; state={state}; screen={screen}");
            }
            wait_for_empty_codex(driver, &prefix, &pane, pid).await;
        }
    }
}

async fn wait_for_empty_codex(driver: &str, prefix: &[String], pane: &str, pid: u32) {
    for _ in 0..100 {
        if matches!(
            codex_terminal_input(Path::new(driver), prefix, pane, pid, None).await,
            Ok(None)
        ) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let result = Command::new(driver)
        .args(prefix)
        .args(["capture-pane", "-p", "-e", "-t", pane])
        .output()
        .unwrap();
    panic!(
        "{driver}: Codex did not reach an empty prompt: {}",
        String::from_utf8_lossy(&result.stdout)
    );
}
