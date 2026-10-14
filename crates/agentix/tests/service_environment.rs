//! Service startup and inherited environment regressions.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "../src/login_environment.rs"]
mod login_environment;

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn check_bootstrap(prefix: &str, expected_calls: usize) {
    let directory = tempfile::tempdir().unwrap();
    let shell = directory.path().join("login-shell");
    let count = directory.path().join("calls");
    script(
        &shell,
        &format!(
            r#"printf '%s\n' "$PPID" >> "$FIXTURE/calls"
{prefix}
printf 'startup noise\n'
exec /bin/sh -c "$2""#
        ),
    );
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentix"))
        .args(["--config", "missing-service-config.toml", "serve"])
        .current_dir(directory.path())
        .env_clear()
        .env("HOME", directory.path())
        .env("PATH", "/usr/bin:/bin")
        .env("FIXTURE", directory.path())
        .env("AGENTIX_LOGIN_SHELL", shell)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(15) {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "bootstrap did not finish: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "missing config must fail after bootstrap"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing-service-config.toml"));
    let calls = std::fs::read_to_string(count)
        .expect("serve must read the login environment before configuration");
    assert_eq!(
        calls,
        format!("{pid}\n").repeat(expected_calls),
        "re-exec must preserve PID and avoid a second successful lookup"
    );
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[test]
fn serve_bootstraps_login_environment_once_before_config() {
    check_bootstrap("", 1);
}

#[test]
fn serve_retries_timed_out_environment_before_loading_config() {
    check_bootstrap(
        r#"if [ ! -f "$FIXTURE/attempted" ]; then
    touch "$FIXTURE/attempted"
    exec /bin/sleep 30
fi"#,
        2,
    );
}

#[test]
fn serve_retries_invalid_environment_before_loading_config() {
    check_bootstrap(
        r#"if [ ! -f "$FIXTURE/attempted" ]; then
    touch "$FIXTURE/attempted"
    printf 'unframed-secret-value'
    exit 0
fi"#,
        2,
    );
}

#[test]
fn non_service_commands_do_not_load_login_environment() {
    let directory = tempfile::tempdir().unwrap();
    let shell = directory.path().join("shell");
    script(&shell, "exit 99");
    let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
        .args(["completions", "fish"])
        .env("AGENTIX_LOGIN_SHELL", shell)
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn initialized_service_reuses_environment_for_codex_and_terminal_commands() {
    let directory = tempfile::tempdir().unwrap();
    let shell = directory.path().join("shell");
    script(&shell, r#"echo unexpected >> "$FIXTURE/calls"; exit 1"#);
    script(
        &directory.path().join("snapshot-codex"),
        r#"/usr/bin/env -0 > "$FIXTURE/codex-env""#,
    );
    script(&directory.path().join("rmux"), "/usr/bin/env -0");
    let output = Command::new("/bin/sh")
        .args([
            "-c",
            "export AGENTIX_SERVICE_ENVIRONMENT_PID=$$; exec \"$@\"",
            "fixture",
        ])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "snapshot_child", "--ignored", "--nocapture"])
        .env_clear()
        .env("HOME", directory.path())
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", directory.path().display()),
        )
        .env("FIXTURE", directory.path())
        .env("AGENTIX_LOGIN_SHELL", shell)
        .env("SNAPSHOT_VALUE", "cached\nwith=equals")
        .env("http_proxy", "http://proxy.test:1001")
        .env("HTTP_PROXY", "http://proxy.test:1003")
        .env("https_proxy", "http://proxy.test:1004")
        .env("no_proxy", "127.0.0.1")
        .env("HTTPS_PROXY", "http://proxy.test:1002")
        .env("NO_PROXY", "localhost")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !directory.path().join("calls").exists(),
        "initialized service must never invoke the login shell again"
    );
}

#[tokio::test]
#[ignore = "subprocess fixture with an isolated service environment"]
async fn snapshot_child() {
    login_environment::reexec().await.unwrap();
    let directory = std::path::PathBuf::from(std::env::var_os("FIXTURE").unwrap());
    assert_eq!(
        std::env::var("SNAPSHOT_VALUE").unwrap(),
        "cached\nwith=equals"
    );
    assert!(std::env::var_os("INHERITED_ONLY").is_none());
    for _ in 0..3 {
        let mut command = tokio::process::Command::new("rmux");
        command.arg("-0");
        let output = agentix_multiplexer::terminal_command_output(&mut command)
            .await
            .unwrap();
        assert!(output.status.success());
        assert_snapshot(&output.stdout);
        if std::env::var_os("PROBE_TMUX").is_some() {
            assert!(agentix_tmux::TmuxDriver::default().probe().await.unwrap());
            assert_snapshot(&std::fs::read(directory.join("tmux-env")).unwrap());
        }
        let endpoint =
            agentix_codex::CodexEndpoint::from_socket_path(&directory.join("upstream.sock"))
                .unwrap();
        // The mock exits after recording its environment instead of hosting a socket.
        let result =
            agentix_codex::UpstreamServer::ensure(&endpoint, Path::new("snapshot-codex")).await;
        assert!(result.is_err());
        assert_snapshot(&std::fs::read(directory.join("codex-env")).unwrap());
    }
}

fn assert_snapshot(bytes: &[u8]) {
    for key in [
        "PATH",
        "SNAPSHOT_VALUE",
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "no_proxy",
        "NO_PROXY",
    ] {
        let expected = format!("{key}={}", std::env::var(key).unwrap());
        assert!(
            bytes.split(|b| *b == 0).any(|v| v == expected.as_bytes()),
            "missing snapshot value for {key}"
        );
    }
}

#[test]
fn bootstrap_shares_full_snapshot_with_service_codex_and_tmux() {
    let directory = tempfile::tempdir().unwrap();
    let shell = directory.path().join("shell");
    script(
        &shell,
        r#"printf '%s\n' "$PPID" >> "$FIXTURE/calls"
export PATH="$FIXTURE:/usr/bin:/bin"
export SNAPSHOT_VALUE='cached
with=equals'
export http_proxy=http://proxy.test:1001
export HTTP_PROXY=http://proxy.test:1003
export https_proxy=http://proxy.test:1004
export no_proxy=127.0.0.1
export HTTPS_PROXY=http://proxy.test:1002
export NO_PROXY=localhost
unset INHERITED_ONLY
export AGENTIX_LOGIN_SHELL="$FIXTURE/no-more-lookups"
exec /bin/sh -c "$2""#,
    );
    script(
        &directory.path().join("snapshot-codex"),
        r#"/usr/bin/env -0 > "$FIXTURE/codex-env""#,
    );
    script(
        &directory.path().join("tmux"),
        r#"/usr/bin/env -0 > "$FIXTURE/tmux-env""#,
    );
    script(&directory.path().join("rmux"), "/usr/bin/env -0");
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "snapshot_child", "--ignored", "--nocapture"])
        .env_clear()
        .env("HOME", directory.path())
        .env("FIXTURE", directory.path())
        .env("PATH", "/usr/bin:/bin")
        .env("AGENTIX_LOGIN_SHELL", shell)
        .env("AGENTIX_SERVICE_ENVIRONMENT_PID", "0")
        .env("INHERITED_ONLY", "remove-me")
        .env("PROBE_TMUX", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("calls")).unwrap(),
        format!("{pid}\n")
    );
}

#[test]
fn serve_waiting_for_login_environment_stops_on_sigterm() {
    let directory = tempfile::tempdir().unwrap();
    let shell = directory.path().join("shell");
    script(&shell, r#"touch "$FIXTURE/entered"; exec /bin/sleep 30"#);
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentix"))
        .args(["--config", "missing.toml", "serve"])
        .env("AGENTIX_LOGIN_SHELL", shell)
        .env("FIXTURE", directory.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !directory.path().join("entered").exists() {
        if started.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("bootstrap did not start");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if started.elapsed() > Duration::from_secs(3) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("bootstrap must stop promptly");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn zsh_login_configuration_is_loaded_once_when_available() {
    check_real_shell("zsh");
}

#[test]
fn fish_login_configuration_is_loaded_once_when_available() {
    check_real_shell("fish");
}

fn check_real_shell(name: &str) {
    let executable = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(name))
        .find(|p| p.is_file());
    let Some(executable) = executable else {
        eprintln!("{name} is not installed; skipping actual shell configuration check");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let config = if name == "fish" {
        let config = directory.path().join("fish");
        std::fs::create_dir(&config).unwrap();
        config.join("config.fish")
    } else {
        directory.path().join(".zprofile")
    };
    let body = if name == "fish" {
        r#"echo call >> "$FIXTURE/calls"
set -gx PATH "$FIXTURE" /usr/bin /bin
set -gx SNAPSHOT_VALUE 'cached
with=equals'
set -gx http_proxy http://proxy.test:1001
set -gx HTTP_PROXY http://proxy.test:1003
set -gx https_proxy http://proxy.test:1004
set -gx no_proxy 127.0.0.1
set -gx HTTPS_PROXY http://proxy.test:1002
set -gx NO_PROXY localhost
set -e INHERITED_ONLY
set -gx AGENTIX_LOGIN_SHELL "$FIXTURE/no-more-lookups"
"#
    } else {
        r#"echo call >> "$FIXTURE/calls"
export PATH="$FIXTURE:/usr/bin:/bin"
export SNAPSHOT_VALUE='cached
with=equals'
export http_proxy=http://proxy.test:1001
export HTTP_PROXY=http://proxy.test:1003
export https_proxy=http://proxy.test:1004
export no_proxy=127.0.0.1
export HTTPS_PROXY=http://proxy.test:1002
export NO_PROXY=localhost
unset INHERITED_ONLY
export AGENTIX_LOGIN_SHELL="$FIXTURE/no-more-lookups"
"#
    };
    std::fs::write(config, body).unwrap();
    script(
        &directory.path().join("snapshot-codex"),
        r#"/usr/bin/env -0 > "$FIXTURE/codex-env""#,
    );
    script(&directory.path().join("rmux"), "/usr/bin/env -0");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "snapshot_child", "--ignored", "--nocapture"])
        .env_clear()
        .env("HOME", directory.path())
        .env("ZDOTDIR", directory.path())
        .env("XDG_CONFIG_HOME", directory.path())
        .env("FIXTURE", directory.path())
        .env("PATH", "/usr/bin:/bin")
        .env("AGENTIX_LOGIN_SHELL", executable)
        .env("INHERITED_ONLY", "remove-me")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("calls")).unwrap(),
        "call\n"
    );
}
