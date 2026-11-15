use std::process::Command;

fn run(config: &str, shell: &str) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, config).unwrap();
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
            "[environment]\nTOKEN = \"a'b\\\\c $HOME\"\nENABLED = true\nEMPTY = ''\n",
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
        "[environment]\nGOOD = 'value'\n'BAD;echo' = 'secret'",
        "[environment]\nGOOD = 'value'\nBAD = ['secret']",
        "[environment]\nBAD = \"secret\\u0000\"",
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
    let output = run("[environment]\nA = 'b'", "unknown");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
#[cfg(unix)]
fn env_detects_actual_shell_and_round_trips_literal_values() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    let value = "a'b\\c $HOME $(touch injected) `touch injected`\n中文\n";
    let mut table = toml::Table::new();
    table.insert("ENV_EXPORT_TEST_VALUE".into(), value.into());
    let mut root = toml::Table::new();
    root.insert("environment".into(), table.into());
    std::fs::write(&config, toml::to_string(&root).unwrap()).unwrap();
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
fn env_rejects_missing_files_and_hides_malformed_toml_values() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .arg("--config")
        .arg(directory.path().join("missing.toml"))
        .args(["env", "--shell", "fish"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let output = run("[environment]\nTOKEN = 'private-test-value", "sh");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-test-value"));
}
