use std::process::Command;

#[test]
fn init_creates_obsidian_configuration_and_bases_without_format_option() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let config_path = dir.path().join("config.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_taskcli"))
        .arg("--config")
        .arg(&config_path)
        .args(["--json", "init", "--root"])
        .arg(dir.path())
        .args(["--directory", "Tasks", "--database"])
        .arg(dir.path().join("tasks.sqlite3"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert!(config["documents"].get("format").is_none());
    assert!(dir.path().join("Tasks/Dashboard.base").is_file());
    assert!(dir.path().join("Tasks/Recent Jobs.base").is_file());
    assert!(!dir.path().join("Tasks/Dashboard.md").exists());
    for command in [
        vec!["context", "--session", "init-test"],
        vec!["obsidian", "connection"],
        vec!["obsidian", "snapshot"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_taskcli"))
            .arg("--config")
            .arg(&config_path)
            .arg("--json")
            .args(command)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value["result"]["documents"].get("format").is_none());
    }
}

#[test]
fn init_help_does_not_advertise_a_format_option() {
    let output = Command::new(env!("CARGO_BIN_EXE_taskcli"))
        .args(["init", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("--format")
    );
}
