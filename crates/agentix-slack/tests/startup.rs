//! Startup synchronization against a reusable Slack CLI process fixture.
#![cfg(unix)]
use agentix_domain::ChannelCommand;
use agentix_slack::SlackCommandSync;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, time::Duration};

fn fixture() -> (tempfile::TempDir, SlackCommandSync) {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("slack cli");
    std::fs::write(&executable, include_str!("fixtures/slack-cli.sh")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        dir.path().join("remote.json"),
        json!({
            "display_information":{"name":"Test"},"settings":{"socket_mode_enabled":true},
            "features":{"slash_commands":[{"command":"/unrelated","description":"Keep"}]}
        })
        .to_string(),
    )
    .unwrap();
    let sync = SlackCommandSync::new(
        executable,
        "A123".into(),
        vec![ChannelCommand::new("sessions", "Browse sessions")],
    );
    (dir, sync)
}

fn assert_projects_removed(dir: &tempfile::TempDir) {
    let projects = std::fs::read_to_string(dir.path().join("projects")).unwrap();
    assert!(!projects.is_empty());
    for project in projects.lines() {
        assert!(
            !std::path::Path::new(project).exists(),
            "temporary CLI project remains: {project}"
        );
    }
}

#[tokio::test]
async fn fetch_merge_install_verify_and_skip_unchanged_updates() {
    let (dir, sync) = fixture();
    assert!(sync.sync("T123").await.unwrap());
    assert_projects_removed(&dir);
    let installed: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("installed.json")).unwrap()).unwrap();
    assert_eq!(
        installed["features"]["slash_commands"][0]["command"],
        "/unrelated"
    );
    assert!(
        installed["features"]["slash_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["command"] == "/sessions")
    );
    assert!(!sync.sync("T123").await.unwrap());
    assert_projects_removed(&dir);
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("app install"))
            .count(),
        1
    );
    assert!(
        calls
            .lines()
            .filter(|line| line.starts_with("manifest info"))
            .count()
            >= 3
    );
}

#[tokio::test]
async fn cli_failures_do_not_leak_output_and_never_attempt_update() {
    let (dir, sync) = fixture();
    std::fs::write(dir.path().join("fail"), "").unwrap();
    let error = sync.sync("T123").await.unwrap_err().to_string();
    assert!(!error.contains("secret-token"));
    assert!(error.contains('7'));
    assert!(!dir.path().join("installed.json").exists());
    assert_projects_removed(&dir);
}

#[tokio::test]
async fn hung_cli_has_bounded_startup_time() {
    let (dir, sync) = fixture();
    std::fs::write(dir.path().join("hang"), "").unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        sync.with_timeout(Duration::from_secs(1)).sync("T123"),
    )
    .await;
    assert!(
        result
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert_projects_removed(&dir);
}

#[tokio::test]
async fn missing_cli_provides_configuration_hint() {
    let sync = SlackCommandSync::new(
        "/nonexistent/agentix-test/slack".into(),
        "A123".into(),
        vec![],
    );
    assert!(
        sync.sync("T123")
            .await
            .unwrap_err()
            .to_string()
            .contains("slack_cli_path")
    );
}

#[allow(dead_code)]
mod support;

#[tokio::test]
async fn adapter_syncs_before_connecting_and_continues_after_sync_failure() {
    use agentix_domain::ChannelAdapter;
    for fail in [false, true] {
        let (dir, sync) = fixture();
        if fail {
            std::fs::write(dir.path().join("fail"), "").unwrap();
        }
        let calls = dir.path().join("calls");
        let installed = dir.path().join("installed.json");
        let server = support::Server::new(move |request| {
            if request.path.ends_with("auth.test") {
                return (
                    200,
                    vec![],
                    json!({"ok":true,"team_id":"T123","user_id":"BOT"}),
                );
            }
            assert!(
                calls.exists(),
                "CLI sync must run before Socket Mode connects"
            );
            assert_eq!(installed.exists(), !fail);
            (200, vec![], json!({"ok":false,"error":"invalid_auth"}))
        })
        .await;
        let adapter = agentix_slack::SlackAdapter::with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            server.url.parse().unwrap(),
            "bot",
            "app",
            vec![],
        )
        .unwrap()
        .with_command_sync(sync);
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            adapter.run(tx, tokio_util::sync::CancellationToken::new()),
        )
        .await;
        assert!(result.unwrap().is_err());
        assert!(dir.path().join("calls").exists());
    }
}

#[tokio::test]
async fn ignores_slack_default_fields_and_command_order_when_already_synced() {
    let (dir, sync) = fixture();
    sync.sync("T123").await.unwrap();
    let path = dir.path().join("remote.json");
    let mut remote: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let commands = remote["features"]["slash_commands"].as_array_mut().unwrap();
    commands.reverse();
    for command in commands {
        if command["command"].as_str().unwrap().starts_with("/agentix") {
            command["url"] = json!("");
            command.as_object_mut().unwrap().remove("should_escape");
        }
    }
    std::fs::write(path, remote.to_string()).unwrap();
    assert!(!sync.sync("T123").await.unwrap());
    assert_projects_removed(&dir);
}

#[tokio::test]
async fn concurrent_changes_abort_and_install_failures_are_not_success() {
    for (mode, message) in [
        ("race", "changed during"),
        ("install-fail", "failed"),
        ("ignore-install", "could not be verified"),
    ] {
        let (dir, sync) = fixture();
        std::fs::write(dir.path().join(mode), "").unwrap();
        std::fs::write(dir.path().join("changed.json"), json!({"display_information":{"name":"Concurrent edit"},"settings":{"socket_mode_enabled":true}}).to_string()).unwrap();
        assert!(
            sync.sync("T123")
                .await
                .unwrap_err()
                .to_string()
                .contains(message)
        );
        assert!(!dir.path().join("installed.json").exists());
        assert_projects_removed(&dir);
        if mode == "race" {
            assert!(
                !std::fs::read_to_string(dir.path().join("calls"))
                    .unwrap()
                    .contains("app install")
            );
        }
    }
}

#[tokio::test]
async fn shutdown_cancels_initialization_before_socket_connect() {
    use agentix_domain::ChannelAdapter;
    let (dir, sync) = fixture();
    std::fs::write(dir.path().join("hang"), "").unwrap();
    let mut server = support::Server::new(|_| {
        (
            200,
            vec![],
            json!({"ok":true,"team_id":"T123","user_id":"BOT"}),
        )
    })
    .await;
    let adapter = agentix_slack::SlackAdapter::with_client(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        server.url.parse().unwrap(),
        "bot",
        "app",
        vec![],
    )
    .unwrap()
    .with_command_sync(sync);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let shutdown = tokio_util::sync::CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let task = tokio::spawn(async move { adapter.run(tx, task_shutdown).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !dir.path().join("calls").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        server
            .requests
            .recv()
            .await
            .unwrap()
            .path
            .ends_with("auth.test")
    );
    assert!(server.requests.try_recv().is_err());
    assert_projects_removed(&dir);
}

#[tokio::test]
async fn reports_actionable_validation_errors_without_cli_output() {
    let (dir, sync) = fixture();
    std::fs::write(dir.path().join("validation-fail"), "").unwrap();
    let error = sync.sync("T123").await.unwrap_err().to_string();
    assert!(error.contains("desc_too_long"), "{error}");
    assert!(error.contains("description"), "{error}");
    assert!(!error.contains("private-token"));
    assert_projects_removed(&dir);
}

#[tokio::test]
async fn repairs_missing_scope_even_when_commands_already_match() {
    let (dir, sync) = fixture();
    sync.sync("T123").await.unwrap();
    let path = dir.path().join("remote.json");
    let mut remote: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    remote["oauth_config"]["scopes"]["bot"] = json!(["chat:write"]);
    std::fs::write(&path, remote.to_string()).unwrap();
    assert!(sync.sync("T123").await.unwrap());
    let repaired: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        repaired["oauth_config"]["scopes"]["bot"],
        json!(["chat:write", "commands"])
    );
}

#[path = "support/claim_lifecycle.rs"]
mod claim_lifecycle;

#[tokio::test]
async fn reports_missing_app_without_exposing_cli_credentials() {
    let (dir, sync) = fixture();
    std::fs::write(dir.path().join("missing-app"), "").unwrap();
    let error = sync.sync("T123").await.unwrap_err().to_string();
    assert!(error.contains("app_not_found"), "{error}");
    assert!(error.contains("channel.slack.app_id"), "{error}");
    assert!(!error.contains("private-token"));
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert!(!calls.contains("app install"));
    assert_projects_removed(&dir);
}

#[tokio::test]
async fn synchronizes_configured_command_affixes_and_skips_identical_menu() {
    let (dir, sync) = fixture();
    let sync =
        sync.with_command_affixes(agentix_slack::CommandAffixes::new("ax-", "-dev").unwrap());
    assert!(sync.sync("T123").await.unwrap());
    assert!(!sync.sync("T123").await.unwrap());
    let remote: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("remote.json")).unwrap())
            .unwrap();
    assert!(
        remote["features"]["slash_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["command"] == "/ax-sessions-dev")
    );
}
