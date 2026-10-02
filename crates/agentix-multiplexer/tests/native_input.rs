//! Isolated real terminal transport; no model provider or developer session.
use agentix_multiplexer::{
    CodexCheckoutChoice, CodexNewSessionOutcome, codex_terminal_input, select_codex_checkout,
    send_codex_new,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

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

struct CodexHomeCleanup(PathBuf, PathBuf);
impl CodexHomeCleanup {
    fn local(binary: &str, root: &Path) -> Option<Self> {
        std::env::var_os("AGENTIX_TEST_CODEX_ENDPOINT")
            .is_none()
            .then(|| Self(PathBuf::from(binary), root.join("codex-home")))
    }
}
impl Drop for CodexHomeCleanup {
    fn drop(&mut self) {
        let _ = Command::new(&self.0)
            .env("CODEX_HOME", &self.1)
            .args(["app-server", "daemon", "stop"])
            .output();
    }
}
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_codex_draft_is_read_and_cleared_only_after_matching_confirmation() {
    if std::env::var_os("AGENTIX_TEST_NATIVE_HOSTS").is_none() {
        return;
    }
    for (driver, style, checkout) in [
        ("tmux", "colored", "checkout"),
        ("rmux", "colored", "checkout"),
        ("tmux", "plain", "checkout"),
        ("rmux", "plain", "checkout"),
        ("tmux", "colored", "legacy"),
        ("rmux", "colored", "legacy"),
        ("tmux", "plain", "legacy"),
        ("rmux", "plain", "legacy"),
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
            "exec {} {} {} 'local draft\nsecond line' codex {style} {checkout}",
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
        let outcome = send_codex_new(Path::new(driver), &prefix, &pane, pid)
            .await
            .unwrap();
        if outcome == CodexNewSessionOutcome::CheckoutChoice {
            assert!(!root.path().join("sent").exists(), "wait for the IM choice");
            let choice = if style == "plain" {
                CodexCheckoutChoice::NewWorktree
            } else {
                CodexCheckoutChoice::CurrentCheckout
            };
            select_codex_checkout(Path::new(driver), &prefix, &pane, pid, choice)
                .await
                .unwrap();
        }
        for _ in 0..30 {
            if root.path().join("sent").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if outcome == CodexNewSessionOutcome::CheckoutChoice {
            assert_eq!(
                std::fs::read_to_string(root.path().join("sent.choice")).unwrap(),
                if style == "plain" {
                    "New worktree"
                } else {
                    "Current checkout"
                }
            );
        }
        assert_eq!(
            std::fs::read_to_string(root.path().join("sent")).unwrap(),
            "/new"
        );
    }
}

/// Opt-in real Codex TUI validation with an isolated home or supplied app-server.
/// Sends only /new and an unsubmitted local draft; no model prompt is submitted.
#[tokio::test]
async fn real_codex_submits_new_from_empty_and_confirmed_draft() {
    let Ok(binary) = std::env::var("AGENTIX_TEST_CODEX_BINARY") else {
        return;
    };
    for driver in ["tmux", "rmux"] {
        let root = tempfile::tempdir().unwrap();
        initialize_test_checkout(root.path());
        let socket = root.path().join("server.sock");
        let socket = socket.to_str().unwrap();
        let command = real_codex_command(Path::new(&binary), root.path());
        let _codex_cleanup = CodexHomeCleanup::local(&binary, root.path());
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
        if startup.contains("Do you trust the contents of this directory?")
            || (startup.contains("Trust this folder?") && startup.contains("Trust and continue"))
        {
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
                        .unwrap_or_else(|error| {
                            let screen = output(
                                driver,
                                &["-S", socket, "capture-pane", "-p", "-e", "-t", &pane],
                            );
                            panic!("{driver}: {error}; draft screen={screen}");
                        })
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
            submit_real_codex_new(
                driver,
                &prefix,
                &pane,
                pid,
                if draft.is_some() {
                    CodexCheckoutChoice::NewWorktree
                } else {
                    CodexCheckoutChoice::CurrentCheckout
                },
            )
            .await;
            wait_for_empty_codex(driver, &prefix, &pane, pid).await;
            if std::env::var_os("AGENTIX_TEST_CODEX_ENDPOINT").is_none() {
                assert_worktree_count(root.path(), if draft.is_some() { 2 } else { 1 });
            }
        }
    }
}

fn assert_worktree_count(root: &Path, expected: usize) {
    let worktrees = output(
        "git",
        &[
            "-C",
            root.to_str().unwrap(),
            "worktree",
            "list",
            "--porcelain",
        ],
    );
    assert_eq!(
        worktrees
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        expected,
        "only the explicit New worktree choice creates a managed checkout"
    );
}

fn initialize_test_checkout(root: &Path) {
    output("git", &["init", "--quiet", root.to_str().unwrap()]);
    output(
        "git",
        &[
            "-C",
            root.to_str().unwrap(),
            "-c",
            "user.name=Agentix Test",
            "-c",
            "user.email=agentix-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--quiet",
            "--allow-empty",
            "-s",
            "-m",
            "test: initialize checkout fixture",
        ],
    );
}

async fn submit_real_codex_new(
    driver: &str,
    prefix: &[String],
    pane: &str,
    pid: u32,
    choice: CodexCheckoutChoice,
) {
    let outcome = send_codex_new(Path::new(driver), prefix, pane, pid)
        .await
        .unwrap_or_else(|error| {
            let screen = Command::new(driver)
                .args(prefix)
                .args(["capture-pane", "-p", "-e", "-t", pane])
                .output()
                .unwrap();
            panic!(
                "{driver}: {error}; screen={}",
                String::from_utf8_lossy(&screen.stdout)
            );
        });
    if std::env::var_os("AGENTIX_TEST_CODEX_ENDPOINT").is_none() {
        assert_eq!(outcome, CodexNewSessionOutcome::CheckoutChoice);
    }
    if outcome == CodexNewSessionOutcome::CheckoutChoice {
        select_codex_checkout(Path::new(driver), prefix, pane, pid, choice)
            .await
            .unwrap();
    }
}

fn real_codex_command(binary: &Path, root: &Path) -> String {
    if let Ok(endpoint) = std::env::var("AGENTIX_TEST_CODEX_ENDPOINT") {
        format!(
            "exec {} --remote {} --no-alt-screen --dangerously-bypass-approvals-and-sandbox -C {}",
            quote(binary),
            quote(Path::new(&endpoint)),
            quote(root)
        )
    } else {
        let codex_home = root.join("codex-home");
        std::fs::create_dir(&codex_home).unwrap();
        std::fs::write(
            codex_home.join("config.toml"),
            format!(
                "model = \"gpt-6-astra\"\nmodel_provider = \"fixture\"\n[features]\nworktrees = true\n[model_providers.fixture]\nname = \"Offline terminal fixture\"\nbase_url = \"http://127.0.0.1:1/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n[projects.{:?}]\ntrust_level = \"trusted\"\n",
                root.canonicalize().unwrap().to_str().unwrap()
            ),
        )
        .unwrap();
        format!(
            "exec env CODEX_HOME={} {} --no-alt-screen --dangerously-bypass-approvals-and-sandbox -C {}",
            quote(&codex_home),
            quote(binary),
            quote(root)
        )
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
