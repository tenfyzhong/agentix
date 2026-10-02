use super::{CodexCheckoutChoice, select_codex_checkout, send_codex_new};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn check(case: &str) {
    let root = tempfile::tempdir().unwrap();
    script(&root.path().join("ps"), "printf '1 1 codex\\n'");
    script(
        &root.path().join("shell"),
        "export PATH=\"$FIXTURE_ROOT:/usr/bin:/bin\"; exec /bin/sh -c \"$2\"",
    );
    script(
        &root.path().join("mux"),
        r#"stage=$(cat "$FIXTURE_ROOT/stage" 2>/dev/null || printf empty)
case "$1" in
  display-message)
    case "$stage" in
      typed) printf 'codex|6|2|0|0\n' ;;
      blocked) printf 'codex|0|0|1|0\n' ;;
      picker)
        if [ "$CASE" = copy ]; then printf 'codex|0|0|1|0\n'
        elif [ "$CASE" = stale ]; then printf 'codex|2|6|0|0\n'
        else printf 'codex|0|0|0|0\n'; fi ;;
      *) printf 'codex|2|2|0|0\n' ;;
    esac ;;
  capture-pane)
    case "$stage" in
      typed) printf 'history\n\n› /new\n\n  gpt-6-astra · Context 0%% used\n' ;;
      picker)
        printf 'Where should the new conversation run?\n\n'
        if [ "$CASE" = changed ]; then
          printf '  1. Current checkout  Keep using the current working directory\n› 2. New worktree  Create an isolated managed checkout\n'
        elif [ "$CASE" = unrelated ]; then
          printf '› 1. Archive session\n  2. Cancel\n'
        else
          printf '› 1. Current checkout  Keep using the current working directory\n  2. New worktree  Create an isolated managed checkout\n'
        fi
        if [ "$CASE" = stale ]; then
          printf '\n\n› \033[2mAsk Codex to do anything\033[22m\n\n  gpt-6-astra · Context 0%% used\n'
        fi ;;
      *) printf 'history\n\n› \033[2mAsk Codex to do anything\033[22m\n\n  gpt-6-astra · Context 0%% used\n' ;;
    esac ;;
  send-keys)
    printf '%s\n' "$*" >> "$FIXTURE_ROOT/keys"
    if [ "$4" = -l ]; then printf typed > "$FIXTURE_ROOT/stage"
    elif [ "$stage" = typed ] && [ "$CASE" != legacy ]; then
      printf picker > "$FIXTURE_ROOT/stage"
    else printf empty > "$FIXTURE_ROOT/stage"; fi ;;
esac"#,
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_control::codex_new_tests::new_session_child",
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
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn codex_new_leaves_checkout_selection_waiting_for_the_user() {
    check("picker");
}

#[test]
fn codex_new_without_picker_does_not_send_an_extra_enter() {
    check("legacy");
}

#[test]
fn codex_new_does_not_confirm_a_picker_left_in_scrollback() {
    check("stale");
}

#[test]
fn codex_new_does_not_confirm_either_selected_checkout() {
    check("changed");
}

#[test]
fn codex_new_does_not_accept_an_unrelated_dialog() {
    check("unrelated");
}

#[test]
fn codex_new_does_not_accept_checkout_selection_in_copy_mode() {
    check("copy");
}

#[test]
fn codex_checkout_cancellation_sends_escape_without_confirming() {
    check("cancel");
}

#[test]
fn codex_checkout_rejects_a_picker_that_was_already_closed() {
    check("expired");
}

#[test]
fn codex_checkout_rechecks_copy_mode_before_submission() {
    check("blocked");
}

#[tokio::test]
#[ignore = "invoked by regression tests with an isolated terminal environment"]
async fn new_session_child() {
    let root = std::env::var("FIXTURE_ROOT").unwrap();
    let case = std::env::var("CASE").unwrap();
    let result = send_codex_new(&Path::new(&root).join("mux"), &[], "%1", 42).await;
    if matches!(case.as_str(), "cancel" | "expired" | "blocked") {
        result.unwrap();
        if case != "cancel" {
            std::fs::write(
                Path::new(&root).join("stage"),
                if case == "expired" {
                    "empty"
                } else {
                    "blocked"
                },
            )
            .unwrap();
        }
        let selected = select_codex_checkout(
            &Path::new(&root).join("mux"),
            &[],
            "%1",
            42,
            if case == "cancel" {
                CodexCheckoutChoice::Cancel
            } else {
                CodexCheckoutChoice::CurrentCheckout
            },
        )
        .await;
        assert_eq!(selected.is_ok(), case == "cancel");
        let keys = std::fs::read_to_string(Path::new(&root).join("keys")).unwrap();
        assert_eq!(
            keys.lines().filter(|line| line.ends_with(" Enter")).count(),
            1
        );
        assert_eq!(
            keys.lines()
                .filter(|line| line.ends_with(" Escape"))
                .count(),
            usize::from(case == "cancel")
        );
        return;
    }
    let keys = std::fs::read_to_string(Path::new(&root).join("keys"))
        .unwrap_or_else(|error| panic!("{error}; submission result: {result:?}"));
    let enters = keys.lines().filter(|line| line.ends_with(" Enter")).count();
    match case.as_str() {
        "picker" | "changed" => {
            result.unwrap();
            assert_eq!(
                enters, 1,
                "no checkout may be selected without a user choice"
            );
            assert_eq!(
                std::fs::read_to_string(Path::new(&root).join("stage")).unwrap(),
                "picker"
            );
        }
        "legacy" | "stale" => {
            result.unwrap();
            assert_eq!(enters, 1);
        }
        _ => {
            assert!(result.is_err(), "unexpected picker state must be reported");
            assert_eq!(enters, 1, "unexpected picker state must not receive Enter");
        }
    }
}
