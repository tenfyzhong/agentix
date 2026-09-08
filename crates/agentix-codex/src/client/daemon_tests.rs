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

#[test]
fn daemon_keeps_original_path_when_login_shell_times_out() {
    let started = std::time::Instant::now();
    check_daemon_path("exec /bin/sleep 30", "/usr/bin:/bin", false);
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
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
