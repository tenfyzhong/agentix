use std::process::Command;

fn run(config: &str, shell: &str) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(path.with_file_name(format!("{}.env", std::path::Path::new(binary()).file_stem().unwrap().to_str().unwrap())), config).unwrap();
    Command::new(binary())
        .arg("--config")
        .arg(path)
        .args(["env", "--shell", shell])
        .env("RUST_LOG", "invalid[filter")
        .output()
        .unwrap()
}

#[test]
fn env_exports_config_without_runtime_initialization() {
    for (shell, expected) in [
        (
            "fish",
            "set -Ux EMPTY '';\nset -Ux ENABLED 'true';\nset -Ux TOKEN 'a\\'b\\\\c $HOME';\n",
        ),
        (
            "bash",
            "export EMPTY='';\nexport ENABLED='true';\nexport TOKEN='a'\\''b\\c $HOME';\n",
        ),
        (
            "powershell",
            "$env:EMPTY = '';\n$env:ENABLED = 'true';\n$env:TOKEN = 'a''b\\c $HOME';\n",
        ),
    ] {
        let output = run(
            "TOKEN = \"a'b\\c $HOME\"\nENABLED = true\nEMPTY = ''\n",
            shell,
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn env_rejects_invalid_entries_without_partial_script() {
    for config in [
        "GOOD = 'value'\nBAD;echo = 'secret'",
        "GOOD = 'value'\nBAD-NAME = 'secret",
        "BAD = \"secret\0\"",
    ] {
        let output = run(config, "fish");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("environment"), "{error}");
        assert!(!error.contains("secret"), "{error}");
    }
}

#[test]
fn env_empty_table_emits_nothing_and_unknown_shell_fails() {
    let output = run("", "fish");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let output = run("A = 'b'", "unknown");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
#[cfg(unix)]
fn env_detects_actual_shell_and_round_trips_literal_values() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    let value = "a'b\\c $HOME $(touch injected) `touch injected` 中文";
    let app = std::path::Path::new(binary()).file_stem().unwrap().to_str().unwrap();
    std::fs::write(config.with_file_name(format!("{app}.env")), format!("ENV_EXPORT_TEST_VALUE={value}\n")).unwrap();
    // fish --no-config disables universal-variable persistence; isolate its config instead.
    for (shell, flags, script) in [
        (
            "bash",
            vec!["--noprofile", "--norc"],
            "eval \"$(\"$ENV_EXPORT_BIN\" --config \"$ENV_EXPORT_CONFIG\" env)\"; printf '%s' \"$ENV_EXPORT_TEST_VALUE\"",
        ),
        (
            "zsh",
            vec!["-f"],
            "eval \"$(\"$ENV_EXPORT_BIN\" --config \"$ENV_EXPORT_CONFIG\" env)\"; printf '%s' \"$ENV_EXPORT_TEST_VALUE\"",
        ),
        (
            "fish",
            vec![],
            "$ENV_EXPORT_BIN --config $ENV_EXPORT_CONFIG env | source; set -q -U ENV_EXPORT_TEST_VALUE; or exit 31; printf '%s' \"$ENV_EXPORT_TEST_VALUE\"",
        ),
    ] {
        if Command::new(shell).arg("--version").output().is_err() {
            continue;
        }
        let output = Command::new(shell)
            .args(flags)
            .args(["-c", script])
            .env("ENV_EXPORT_BIN", binary())
            .env("ENV_EXPORT_CONFIG", &config)
            .env("SHELL", "/bin/incorrect-login-shell")
            .env("HOME", directory.path())
            .env("XDG_CONFIG_HOME", directory.path())
            .env_remove("ENV_EXPORT_TEST_VALUE")
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), value, "{shell}");
        assert!(!directory.path().join("injected").exists());
        if shell == "fish" {
            let persisted = Command::new("fish")
                .args(["-c", "set -q -U ENV_EXPORT_TEST_VALUE; or exit 31; printf '%s' \"$ENV_EXPORT_TEST_VALUE\""])
                .env("HOME", directory.path())
                .env("XDG_CONFIG_HOME", directory.path())
                .env_remove("ENV_EXPORT_TEST_VALUE")
                .output().unwrap();
            assert!(persisted.status.success());
            assert_eq!(String::from_utf8(persisted.stdout).unwrap(), value);
        }
    }
}

#[test]
fn env_rejects_missing_files_and_hides_malformed_values() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .arg("--config")
        .arg(directory.path().join("missing.toml"))
        .args(["env", "--shell", "fish"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let output = run("BAD-TOKEN = 'private-test-value", "sh");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-test-value"));
}

#[test]
fn env_reads_dedicated_file_without_toml() {
    let dir = tempfile::tempdir().unwrap();
    let app = std::path::Path::new(binary()).file_stem().unwrap().to_str().unwrap();
    std::fs::write(dir.path().join(format!("{app}.env")), "LOCAL_ENV_TEST=literal\n").unwrap();
    let output = Command::new(binary()).arg("--config").arg(dir.path().join("missing.toml"))
        .args(["env", "--shell", "sh"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "export LOCAL_ENV_TEST='literal';\n");
}

#[test]
fn env_expands_home_paths_and_keeps_credentials_literal() {
    let output = run("CODEX_HOME=~\nOPENAI_API_KEY=~/literal\n", "sh");
    assert!(output.status.success(), "{output:?}");
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap();
    let expected = format!("export CODEX_HOME='{}';\nexport OPENAI_API_KEY='~/literal';\n", home.replace('\'', "'\\''"));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}

#[test]
fn env_does_not_export_global_state_home() {
    let output = run("XDG_STATE_HOME=~/private-state\nRUST_LOG=trace\nOTHER=retained\n", "fish");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "set -Ux OTHER 'retained';\n");
}
