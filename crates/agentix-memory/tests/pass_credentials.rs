// Exercise pass credentials with an isolated executable and environment.
#![cfg(unix)]
use agentix_memory::{HttpProvider, ProviderConfig};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn pass_credentials_are_cached_and_fail_closed() {
    if let Ok(mode) = std::env::var("TASKIX_PASS_TEST_MODE") {
        let mut config: ProviderConfig = serde_json::from_value(serde_json::json!({
            "base_url":"http://localhost:9/v1", "api_key_pass":"folder/key with spaces"
        }))
        .unwrap();
        if mode == "env" || mode == "direct" {
            config.api_key_pass = None;
            config.api_key_env = Some("TASKIX_PASS_TEST_KEY".into());
            HttpProvider::new(config).unwrap();
        } else if mode == "ok" {
            HttpProvider::new(config.clone()).unwrap();
            HttpProvider::new(config).unwrap();
            assert_eq!(
                fs::read_to_string(std::env::var("TASKIX_PASS_TEST_COUNT").unwrap()).unwrap(),
                "call\n"
            );
        } else {
            let error = HttpProvider::new(config)
                .err()
                .expect("must reject invalid credentials");
            assert!(!format!("{error:#}").contains("secret-output"));
        }
        return;
    }
    for mode in [
        "ok", "env", "direct", "fail", "empty", "invalid", "large", "missing",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("pass");
        fs::write(&script, "#!/bin/sh\n[ \"$1\" = show ] && [ \"$2\" = -- ] && [ \"$3\" = 'folder/key with spaces' ] || exit 3\necho call >> \"$TASKIX_PASS_TEST_COUNT\"\ncase \"$TASKIX_PASS_TEST_MODE\" in\nok|env) printf 'test-key\\r\\nmetadata\\n';;\nfail) echo secret-output >&2; exit 1;;\nempty) printf '\\nsecret-output';;\ninvalid) printf 'secret-output\\001';;\nlarge) /usr/bin/head -c 70000 /dev/zero;;\nesac\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        if mode == "missing" {
            fs::remove_file(&script).unwrap();
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .env_remove("TASKIX_PASS_TEST_KEY")
            .env("TASKIX_PASS_TEST_KEY_PASS", "folder/key with spaces");
        if mode == "direct" {
            command.env("TASKIX_PASS_TEST_KEY", "direct-key");
            fs::remove_file(&script).unwrap();
        }
        let output = command
            .args([
                "--exact",
                "pass_credentials_are_cached_and_fail_closed",
                "--nocapture",
            ])
            .env("TASKIX_PASS_TEST_MODE", mode)
            .env("PATH", dir.path())
            .env("TASKIX_PASS_TEST_COUNT", dir.path().join("count"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn pass_and_environment_sources_are_mutually_exclusive() {
    let config: ProviderConfig = serde_json::from_value(serde_json::json!({
        "base_url":"http://localhost:9/v1", "api_key_pass":"folder/key", "api_key_env":"EXAMPLE"
    }))
    .unwrap();
    let error = HttpProvider::new(config).err().unwrap();
    assert!(error.to_string().contains("mutually exclusive"));
}
