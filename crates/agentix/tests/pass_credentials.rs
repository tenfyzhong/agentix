// Isolated fake pass: never read a developer's password store.
#![cfg(unix)]
use agentix::Config;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn selected_channel_resolves_and_caches_pass_credentials() {
    if let Ok(mode) = std::env::var("AGENTIX_PASS_TEST_MODE") {
        for (kind, fields) in [
            ("telegram", "token_pass='test/credential'"),
            ("feishu", "app_id='app'\napp_secret_pass='test/credential'"),
            (
                "slack",
                "bot_token_pass='test/credential'\napp_token_pass='test/credential'",
            ),
        ] {
            let text = format!(
                "[agent]\nkind='codex'\n[storage]\npath='/tmp/test.db'\n[channel]\nkind='{kind}'\n[channel.{kind}]\n{fields}\n"
            );
            let config = Config::from_toml(&text).unwrap();
            if kind == "telegram" {
                assert!(
                    !std::path::Path::new(&std::env::var("AGENTIX_PASS_COUNT").unwrap()).exists(),
                    "loading configuration must not decrypt"
                );
            }
            let values = match kind {
                "telegram" => vec![config.channel.telegram.unwrap().resolve_token()],
                "feishu" => vec![config.channel.feishu.unwrap().resolve_app_secret()],
                _ => {
                    let slack = config.channel.slack.unwrap();
                    vec![slack.resolve_bot_token(), slack.resolve_app_token()]
                }
            };
            for value in values {
                if mode == "ok" {
                    assert_eq!(value.unwrap(), "test-secret");
                } else {
                    let error = format!("{:#}", value.unwrap_err());
                    assert!(!error.contains("private-output"));
                    assert!(error.contains("pass"));
                }
            }
        }
        if mode == "ok" {
            assert_eq!(
                fs::read_to_string(std::env::var("AGENTIX_PASS_COUNT").unwrap()).unwrap(),
                "call\n"
            );
        }
        return;
    }
    for mode in ["ok", "fail"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pass");
        fs::write(&path, "#!/bin/sh\necho call >> \"$AGENTIX_PASS_COUNT\"\n[ \"$1\" = show ] && [ \"$2\" = -- ] && [ \"$3\" = test/credential ] || exit 3\nif [ \"$AGENTIX_PASS_TEST_MODE\" = fail ]; then echo private-output >&2; exit 1; fi\nprintf 'test-secret\\nmetadata\\n'\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "selected_channel_resolves_and_caches_pass_credentials",
                "--nocapture",
            ])
            .env("AGENTIX_PASS_TEST_MODE", mode)
            .env("AGENTIX_PASS_COUNT", dir.path().join("count"))
            .env("PATH", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
