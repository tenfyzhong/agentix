use crate::{inspect_terminal_interaction, respond_terminal_interaction};
use agentix_domain::{AgentKind, TerminalInteractionResponse as Response};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn native_interactions_only_submit_explicit_answers_to_the_same_live_dialog() {
    for case in [
        "choice", "changed", "closed", "copy", "yes", "no", "cancel", "unknown", "pid",
    ] {
        let root = tempfile::tempdir().unwrap();
        script(
            &root.path().join("ps"),
            "if [ -f \"$FIXTURE_ROOT/pid_changed\" ]; then printf '1 2 codex\\n'; else printf '1 1 codex\\n'; fi",
        );
        script(
            &root.path().join("shell"),
            "export PATH=\"$FIXTURE_ROOT:/usr/bin:/bin\"; exec /bin/sh -c \"$2\"",
        );
        script(
            &root.path().join("mux"),
            r#"
stage=$(cat "$FIXTURE_ROOT/stage" 2>/dev/null || printf dialog)
selected=$(cat "$FIXTURE_ROOT/selected" 2>/dev/null || printf 1)
case "$1" in
display-message)
  if [ "$stage" = copy ]; then printf 'codex|0|1|0\n'; else printf 'codex|0|0|0\n'; fi ;;
capture-pane)
  if [ "$stage" = closed ]; then printf '› draft\n';
  elif [ "$CASE" = yes ] || [ "$CASE" = no ]; then printf 'Continue the operation? [y/N]\n';
  elif [ "$CASE" = unknown ]; then printf 'Custom editor\n [x] permission\n Enter submit · Esc cancel\n';
  else
    if [ "$stage" = changed ]; then printf 'A changed dialog\n\n'; else printf 'A future native action\n\n'; fi
    for item in 1 2 3; do
      if [ "$item" = "$selected" ]; then printf '› '; else printf '  '; fi
      printf '%s. Option %s\n' "$item" "$item"
    done
    printf '\nEnter select · Esc cancel\n'
  fi ;;
send-keys)
  printf '%s\n' "$4" >> "$FIXTURE_ROOT/keys"
  case "$4" in
    Down) expr "$selected" + 1 > "$FIXTURE_ROOT/selected" ;;
    Up) expr "$selected" - 1 > "$FIXTURE_ROOT/selected" ;;
    *) printf closed > "$FIXTURE_ROOT/stage" ;;
  esac ;;
esac
"#,
        );
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "interaction_tests::terminal_interaction_child",
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
#[ignore = "isolated child of the native interaction regression test"]
async fn terminal_interaction_child() {
    let root = std::env::var("FIXTURE_ROOT").unwrap();
    let root = Path::new(&root);
    let command = root.join("mux");
    let case = std::env::var("CASE").unwrap();
    let expected = inspect_terminal_interaction(&command, &[], "%9", 42, AgentKind::Codex)
        .await
        .unwrap()
        .unwrap();
    assert!(!root.join("keys").exists(), "inspection must not send keys");
    if ["changed", "closed", "copy"].contains(&case.as_str()) {
        std::fs::write(root.join("stage"), &case).unwrap();
    }
    if case == "pid" {
        std::fs::write(root.join("pid_changed"), "").unwrap();
    }
    let response = match case.as_str() {
        "yes" => Response::Choice(0),
        "no" => Response::Choice(1),
        "cancel" => Response::Cancel,
        _ => Response::Choice(2),
    };
    let result = respond_terminal_interaction(
        &command,
        &[],
        "%9",
        42,
        AgentKind::Codex,
        &expected,
        response,
    )
    .await;
    let keys = std::fs::read_to_string(root.join("keys")).unwrap_or_default();
    match case.as_str() {
        "choice" => {
            result.unwrap();
            assert_eq!(keys, "Down\nDown\nEnter\n");
        }
        "yes" => {
            result.unwrap();
            assert_eq!(keys, "y\n");
        }
        "no" => {
            result.unwrap();
            assert_eq!(keys, "n\n");
        }
        "cancel" => {
            result.unwrap();
            assert_eq!(keys, "Escape\n");
        }
        _ => {
            assert!(result.is_err());
            assert!(keys.is_empty(), "invalid interaction must not send keys");
        }
    }
}
