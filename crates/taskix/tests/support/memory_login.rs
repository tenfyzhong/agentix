use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

struct LoginDaemon(std::process::Child);
impl Drop for LoginDaemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn check_login_service(shell_body: &str, inherited_key: Option<&str>, credentials_available: bool) {
    let cli = Cli::new();
    let config = cli.dir.path().join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("\n[memory]\nenabled = true\n[memory.providers.openai]\nbase_url = 'http://127.0.0.1:9/v1'\napi_key_env = 'TASKIX_LOGIN_TEST_SECRET'\n[memory.agent]\nmodel = 'gpt-6-astra'\n[memory.service]\npoll_interval_ms = 50\n");
    std::fs::write(config, text).unwrap();
    let shell = cli.dir.path().join("login shell");
    std::fs::write(&shell, format!("#!/bin/sh\n{shell_body}\n")).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    let count = cli.dir.path().join("shell calls");
    let parent = cli.dir.path().join("shell parent");
    let log_path = cli.dir.path().join("login.log");
    let mut command = cli.command(&["memory", "serve"]);
    command
        .env_clear()
        .env("HOME", cli.dir.path())
        .env("PATH", "/usr/bin:/bin")
        .env("TASKIX_LOGIN_SHELL", &shell)
        .env("LOGIN_COUNT", &count)
        .env("LOGIN_PARENT", &parent)
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log_path).unwrap());
    if let Some(key) = inherited_key {
        command.env("TASKIX_LOGIN_TEST_SECRET", key);
    }
    let mut daemon = LoginDaemon(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        let output = cli.run(&["memory", "status"]);
        if output.status.success() {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            if value["result"]["online"] == true {
                break value["result"].clone();
            }
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "{}",
            std::fs::read_to_string(&log_path).unwrap()
        );
        assert!(
            Instant::now() < deadline,
            "memory service did not become ready"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        status["provider_errors"].as_array().unwrap().is_empty(),
        credentials_available,
        "{status}"
    );
    assert_eq!(std::fs::read_to_string(&count).unwrap(), "call\n");
    assert_eq!(
        std::fs::read_to_string(&parent).unwrap().trim(),
        daemon.0.id().to_string()
    );
    cli.ok(&["memory", "reload"]);
    assert_eq!(std::fs::read_to_string(&count).unwrap(), "call\n");
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(
        !log.contains("secret-shell-value"),
        "credentials leaked to log"
    );
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &daemon.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = daemon.0.try_wait().unwrap() {
            assert!(status.success(), "service did not exit cleanly: {status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "service did not respond to SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(cli.ok(&["memory", "status"])["online"], false);
}

const RECORD_CALL: &str =
    "printf 'call\\n' >> \"$LOGIN_COUNT\"; printf '%s\\n' \"$PPID\" > \"$LOGIN_PARENT\"";

#[test]
fn memory_serve_loads_login_credentials_once_and_preserves_pid() {
    check_login_service(
        &format!(
            "{RECORD_CALL}\nprintf 'startup banner\\n'\nexport TASKIX_LOGIN_TEST_SECRET='secret-shell-value'\nexec /bin/sh -c \"$2\""
        ),
        None,
        true,
    );
}

#[test]
fn memory_serve_honors_login_shell_unset() {
    check_login_service(
        &format!("{RECORD_CALL}\nunset TASKIX_LOGIN_TEST_SECRET\nexec /bin/sh -c \"$2\""),
        Some("inherited-key"),
        false,
    );
}

#[test]
fn memory_serve_keeps_inherited_credentials_when_login_shell_fails() {
    check_login_service(
        &format!("{RECORD_CALL}\nexit 1"),
        Some("inherited-key"),
        true,
    );
}

#[test]
fn memory_serve_keeps_inherited_credentials_when_login_output_is_malformed() {
    check_login_service(
        &format!(
            "{RECORD_CALL}\nprintf '\\0taskix-login-environment\\0TASKIX_LOGIN_TEST_SECRET=broken'"
        ),
        Some("inherited-key"),
        true,
    );
}

#[test]
fn memory_serve_keeps_inherited_credentials_when_login_marker_is_missing() {
    check_login_service(
        &format!("{RECORD_CALL}\nprintf 'startup output only\\n'"),
        Some("inherited-key"),
        true,
    );
}

#[test]
fn memory_serve_bounds_login_shell_timeout() {
    check_login_service(
        &format!("{RECORD_CALL}\nexec /bin/sleep 30"),
        Some("inherited-key"),
        true,
    );
}
