use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn native_exit_submits_only_to_an_unchanged_codex_composer() {
    for case in [
        "exit", "draft", "copy", "dead", "shell", "pid", "changed", "dialog",
    ] {
        let root = tempfile::tempdir().unwrap();
        script(
            &root.path().join("ps"),
            r#"if [ "$CASE" = pid ] || [ -f "$FIXTURE_ROOT/changed" ]; then printf '1 2 codex\n'; else printf '1 1 codex\n'; fi"#,
        );
        script(
            &root.path().join("shell"),
            r#"export PATH="$FIXTURE_ROOT:/usr/bin:/bin"; exec /bin/sh -c "$2""#,
        );
        script(
            &root.path().join("mux"),
            r#"
case "$1" in
  display-message)
    if [ "$CASE" = shell ]; then printf 'zsh|2|1|0|0\n';
    elif [ "$CASE" = copy ]; then printf 'codex|2|1|1|0\n';
    elif [ "$CASE" = dead ]; then printf 'codex|2|1|0|1\n';
    elif [ "$CASE" = dialog ]; then printf 'Confirm an operation?\n';
    elif [ -f "$FIXTURE_ROOT/typed" ]; then printf 'codex|7|1|0|0\n';
    elif [ "$CASE" = draft ]; then printf 'codex|7|1|0|0\n';
    else printf 'codex|2|1|0|0\n'; fi ;;
  capture-pane)
    if [ "$CASE" = dialog ]; then printf 'Confirm an operation?\n';
    elif [ -f "$FIXTURE_ROOT/typed" ]; then printf '\n› /exit\n\n  gpt-6-astra · Context 0% used\n';
    elif [ "$CASE" = draft ]; then printf '\n› draft\n\n  gpt-6-astra · Context 0% used\n';
    else printf '\n› \n\n  gpt-6-astra · Context 0% used\n'; fi ;;
  send-keys)
    printf '%s\n' "$4" >> "$FIXTURE_ROOT/keys"
    [ "$4" != -l ] || printf '%s\n' "$5" >> "$FIXTURE_ROOT/keys"
    if [ "$4" = -l ]; then touch "$FIXTURE_ROOT/typed"; [ "$CASE" != changed ] || touch "$FIXTURE_ROOT/changed"; fi ;;
esac
"#,
        );
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_exit_tests::native_exit_child",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("FIXTURE_ROOT", root.path())
            .env("AGENTIX_LOGIN_SHELL", root.path().join("shell"))
            .env("CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
#[ignore = "isolated child of the native exit regression test"]
async fn native_exit_child() {
    let root = std::env::var("FIXTURE_ROOT").unwrap();
    let root = Path::new(&root);
    let case = std::env::var("CASE").unwrap();
    let result = crate::native_control::send_codex_exit(&root.join("mux"), &[], "%9", 42).await;
    let keys = std::fs::read_to_string(root.join("keys")).unwrap_or_default();
    if case == "exit" {
        result.unwrap();
        assert_eq!(keys, "-l\n/exit\nEnter\n");
    } else {
        assert!(result.is_err());
        assert_eq!(keys, if case == "changed" { "-l\n/exit\n" } else { "" });
    }
}
