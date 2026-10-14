use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn check_daemon_path(shell_body: &str, expected_path: &str, bare_command: bool) {
    check_daemon_environment(shell_body, expected_path, bare_command, "preserved\nunset");
}

fn check_daemon_environment(
    shell_body: &str,
    expected_path: &str,
    bare_command: bool,
    expected_env: &str,
) {
    let directory = tempfile::tempdir().unwrap();
    let shell = directory.path().join("login shell");
    let codex = directory.path().join("fake-codex");
    let result = directory.path().join("result");
    script(&shell, shell_body);
    script(
        &codex,
        "printf '%s\\n' \"$PATH\" \"${KEEP_ME-unset}\" \"${SHELL_ONLY-unset}\" \"$*\" > \"$RESULT\"",
    );
    let path = expected_path.replace("FIXTURE", &directory.path().display().to_string());
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "client::daemon_tests::daemon_start_child",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("HOME", directory.path())
        .env("PATH", "/usr/bin:/bin")
        .env("AGENTIX_LOGIN_SHELL", &shell)
        .env("FIXTURE", directory.path())
        .env("RESULT", &result)
        .env("KEEP_ME", "preserved")
        .env(
            "CODEX_TEST_COMMAND",
            if bare_command {
                Path::new("fake-codex")
            } else {
                &codex
            },
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(result).unwrap(),
        format!("{path}\n{expected_env}\napp-server daemon start\n")
    );
}

#[test]
fn daemon_inherits_login_environment_despite_minimal_service_environment() {
    check_daemon_environment(
        "test \"$1\" = '-lc' || exit 2\nprintf 'shell startup output\\n'\nexport PATH=\"$FIXTURE:/usr/bin:/bin\"\nexport SHELL_ONLY=private\nexec /bin/sh -c \"$2\"",
        "FIXTURE:/usr/bin:/bin",
        false,
        "preserved\nprivate",
    );
}

#[test]
fn daemon_resolves_bare_codex_using_login_path() {
    check_daemon_path(
        "export PATH=\"$FIXTURE:/usr/bin:/bin\"\nexec /bin/sh -c \"$2\"",
        "FIXTURE:/usr/bin:/bin",
        true,
    );
}

#[test]
fn daemon_keeps_original_path_when_login_shell_fails() {
    check_daemon_path("/bin/sh -c \"$2\"\nexit 1", "/usr/bin:/bin", false);
}

#[test]
fn daemon_keeps_original_path_when_login_shell_only_prints_startup_output() {
    check_daemon_path("printf 'welcome\\n'", "/usr/bin:/bin", false);
}

#[test]
fn daemon_preserves_whitespace_in_login_path() {
    check_daemon_path(
        "export PATH=\"$FIXTURE/directory with spaces:/usr/bin:/bin\"\nexec /bin/sh -c \"$2\"",
        "FIXTURE/directory with spaces:/usr/bin:/bin",
        false,
    );
}

#[test]
fn daemon_preserves_an_explicitly_empty_login_path() {
    check_daemon_path("export PATH=''\nexec /bin/sh -c \"$2\"", "", false);
}

// Run in a subprocess so environment fixtures never modify the test runner.
#[tokio::test]
#[ignore = "invoked by daemon environment tests with an isolated environment"]
async fn daemon_start_child() {
    let command = std::env::var_os("CODEX_TEST_COMMAND").unwrap();
    super::start_daemon(Path::new(&command)).await.unwrap();
}

#[test]
fn daemon_inherits_exported_values_with_newlines_equals_and_empty_values() {
    check_daemon_environment(
        "export KEEP_ME=''\nexport SHELL_ONLY='first=line\nsecond line'\nexec /bin/sh -c \"$2\"",
        "/usr/bin:/bin",
        false,
        "\nfirst=line\nsecond line",
    );
}

#[test]
fn daemon_honors_unset_variables_and_does_not_import_shell_locals() {
    check_daemon_environment(
        "unset KEEP_ME\nSHELL_ONLY=local\nexec /bin/sh -c \"$2\"",
        "/usr/bin:/bin",
        false,
        "unset\nunset",
    );
}

#[test]
fn daemon_keeps_entire_original_environment_when_shell_output_is_malformed() {
    check_daemon_path(
        "printf '\\0agentix-login-environment\\0PATH=/invalid\\0broken\\0'",
        "/usr/bin:/bin",
        false,
    );
}

#[test]
fn daemon_keeps_entire_original_environment_when_shell_output_is_truncated() {
    check_daemon_path(
        "printf '\\0agentix-login-environment\\0PATH=/invalid'",
        "/usr/bin:/bin",
        false,
    );
}

#[test]
fn daemon_login_timeout_does_not_spawn_codex() {
    check_login_timeout("daemon");
}

#[test]
fn upstream_login_timeout_does_not_spawn_codex() {
    check_login_timeout("upstream");
}

#[test]
fn deferred_daemon_recovers_login_environment_after_timeout() {
    check_login_timeout("recovery");
}

fn check_login_timeout(mode: &str) {
    let directory = tempfile::tempdir().unwrap();
    let shell = directory.path().join("login-shell");
    let codex = directory.path().join("codex");
    script(
        &shell,
        r#"if [ ! -f "$FIXTURE/attempted" ]; then
    touch "$FIXTURE/attempted"
    exec /bin/sleep 30
fi
export PATH="$FIXTURE:/usr/bin:/bin"
export SHELL_ONLY=recovered
unset KEEP_ME
exec /bin/sh -c "$2""#,
    );
    script(
        &codex,
        r#"printf '%s\n' "$PATH" "${KEEP_ME-unset}" "${SHELL_ONLY-unset}" >> "$FIXTURE/started""#,
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "client::daemon_tests::login_timeout_child",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("HOME", directory.path())
        .env("PATH", "/usr/bin:/bin")
        .env("KEEP_ME", "must-be-unset")
        .env("AGENTIX_LOGIN_SHELL", &shell)
        .env("FIXTURE", directory.path())
        .env("TIMEOUT_TEST_MODE", mode)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

use super::test_support as support;

#[tokio::test]
#[ignore = "invoked by login timeout tests with an isolated environment"]
async fn login_timeout_child() {
    use agentix_core::{AgentAdapter, AgentError, DeferredAgent};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let directory = std::path::PathBuf::from(std::env::var_os("FIXTURE").unwrap());
    let command = directory.join("codex");
    let marker = directory.join("started");
    let started = Instant::now();
    match std::env::var("TIMEOUT_TEST_MODE").unwrap().as_str() {
        "daemon" => {
            let error = super::start_daemon(&command)
                .await
                .expect_err("must reject timeout");
            assert!(
                error
                    .to_string()
                    .contains("login environment lookup timed out"),
                "{error}"
            );
            assert!(
                !marker.exists(),
                "Codex started with the service environment"
            );
        }
        "upstream" => {
            let endpoint =
                crate::CodexEndpoint::from_socket_path(&directory.join("upstream.sock")).unwrap();
            let result = crate::UpstreamServer::ensure(&endpoint, &command).await;
            let error = result.err().expect("must reject timeout");
            assert!(
                !crate::is_fatal_startup_error(&error),
                "service must retry: {error:#}"
            );
            assert!(
                error
                    .to_string()
                    .contains("login environment lookup timed out"),
                "{error:#}"
            );
            assert!(
                !marker.exists(),
                "upstream started with the service environment"
            );
        }
        "recovery" => {
            let server = support::MockCodexAppServer::start();
            let endpoint = server.endpoint();
            let deferred = DeferredAgent::new("Codex", String::new(), move || {
                let command = command.clone();
                let endpoint = endpoint.clone();
                async move {
                    super::start_daemon(&command)
                        .await
                        .map_err(|e| AgentError::Unavailable(e.to_string()))?;
                    let client = super::CodexClient::connect(endpoint)
                        .await
                        .map_err(|e| AgentError::Unavailable(e.to_string()))?;
                    Ok(Arc::new(client) as Arc<dyn AgentAdapter>)
                }
            });
            let mut events = deferred.subscribe();
            tokio::time::timeout(Duration::from_secs(12), events.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                std::fs::read_to_string(&marker).unwrap(),
                format!("{}:/usr/bin:/bin\nunset\nrecovered\n", directory.display()),
                "only the recovered login environment may reach Codex"
            );
            // The parent service environment must remain unchanged.
            assert_eq!(std::env::var("PATH").unwrap(), "/usr/bin:/bin");
            assert_eq!(std::env::var("KEEP_ME").unwrap(), "must-be-unset");
        }
        mode => panic!("unknown timeout test mode: {mode}"),
    }
    assert!(started.elapsed() < Duration::from_secs(12));
}
