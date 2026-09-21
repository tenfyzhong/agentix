use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn check(case: &str, shell_body: &str) {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("bin with spaces");
    std::fs::create_dir(&bin).unwrap();
    let inherited = directory.path().join("inherited");
    std::fs::create_dir(&inherited).unwrap();
    script(&inherited.join("fake-rmux"), "printf wrong-program; exit 0");
    let shell = directory.path().join("login shell");
    script(
        &shell,
        &format!("printf invoked > \"$SHELL_MARKER\"\n{shell_body}"),
    );
    script(
        &bin.join("fake-rmux"),
        "printf '%s\\n' \"$@\"; test -z \"${TMUX+x}${TMUX_PANE+x}\"",
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_control::command_tests::command_child",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env(
            "PATH",
            format!(
                "{}:{}:/usr/bin:/bin",
                inherited.display(),
                directory.path().display()
            ),
        )
        .env(
            "AGENTIX_LOGIN_SHELL",
            if case == "relative" {
                Path::new("login shell")
            } else {
                &shell
            },
        )
        .env("FIXTURE_BIN", bin)
        .env("CASE", case)
        .env("SHELL_MARKER", directory.path().join("shell-invoked"))
        .env("TMUX", "unrelated-server")
        .env("TMUX_PANE", "%99")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(directory.path().join("shell-invoked").exists());
}

#[test]
fn native_command_prefers_login_path_and_preserves_literal_arguments() {
    check(
        "fallback",
        "printf 'startup noise\\n'; export PATH=\"$FIXTURE_BIN:/usr/bin:/bin\"; exec /bin/sh -c \"$2\"",
    );
}

#[test]
fn native_command_does_not_retry_an_executed_failure() {
    check(
        "exit",
        "export PATH=\"$FIXTURE_BIN:/usr/bin:/bin\"; exec /bin/sh -c \"$2\"",
    );
}

#[test]
fn native_command_reports_missing_explicit_path() {
    check(
        "explicit",
        "export PATH=\"$FIXTURE_BIN:/usr/bin:/bin\"; exec /bin/sh -c \"$2\"",
    );
}

#[test]
fn native_command_reports_missing_command_when_shell_output_is_invalid() {
    check("invalid", "printf 'startup noise\\n'");
}

#[test]
fn native_command_bounds_login_shell_timeout() {
    let started = std::time::Instant::now();
    check("invalid", "exec /bin/sleep 30");
    assert!(started.elapsed() < std::time::Duration::from_secs(8));
}

#[test]
fn native_command_rejects_failed_login_shell() {
    check(
        "invalid",
        "export PATH=\"$FIXTURE_BIN:/usr/bin:/bin\"; /bin/sh -c \"$2\"; exit 1",
    );
}

#[test]
fn native_command_explicit_executable_inherits_login_path() {
    check(
        "absolute",
        "export PATH=\"$FIXTURE_BIN:/usr/bin:/bin\"; exec /bin/sh -c \"$2\"",
    );
}

#[test]
fn native_command_supports_login_shell_command_name() {
    check(
        "relative",
        "export PATH=\"$FIXTURE_BIN:/usr/bin:/bin\"; exec /bin/sh -c \"$2\"",
    );
}

#[tokio::test]
#[ignore = "invoked in a child process with an isolated service environment"]
async fn command_child() {
    match std::env::var("CASE").unwrap().as_str() {
        "fallback" | "relative" => {
            let result = super::run(
                Path::new("fake-rmux"),
                &["-L".into(), "server name".into()],
                &["send-keys", "%1", "literal '$HOME'; $(exit 7) \\"],
            )
            .await
            .unwrap();
            assert_eq!(
                result,
                "-L\nserver name\nsend-keys\n%1\nliteral '$HOME'; $(exit 7) \\\n"
            );
        }
        "absolute" => {
            let result = super::run(Path::new("/bin/sh"), &[], &["-c", "printf '%s' \"$PATH\""])
                .await
                .unwrap();
            assert_eq!(
                result,
                format!("{}:/usr/bin:/bin", std::env::var("FIXTURE_BIN").unwrap())
            );
        }
        "exit" => {
            let result = super::run(
                Path::new("sh"),
                &[],
                &["-c", "printf deliberate-failure >&2; exit 7"],
            )
            .await
            .unwrap_err();
            assert!(result.to_string().contains("deliberate-failure"));
        }
        "explicit" => {
            let result = super::run(Path::new("/missing/fake-rmux"), &[], &[])
                .await
                .unwrap_err();
            assert!(
                result.to_string().contains("/missing/fake-rmux"),
                "{result}"
            );
        }
        "invalid" => {
            let result = super::run(Path::new("fake-rmux"), &[], &[])
                .await
                .unwrap_err();
            assert!(result.to_string().contains("login shell PATH"), "{result}");
        }
        case => panic!("unexpected case: {case}"),
    }
}
