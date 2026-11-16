fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_taskix")
}
include!("../../agentix-task/tests/support/env_cli.rs");

#[test]
fn env_honors_taskix_config_and_rejects_json() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    std::fs::write(directory.path().join("taskix.env"), "TASKIX_EXAMPLE = 42\n").unwrap();
    let output = Command::new(binary())
        .args(["env", "--shell", "sh"])
        .env("TASKIX_CONFIG", &config)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "export TASKIX_EXAMPLE='42';\n"
    );
    let output = Command::new(binary())
        .args(["env", "--json"])
        .env("TASKIX_CONFIG", &config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn ordinary_command_loads_local_defaults_and_preserves_inherited_override() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let file_db = dir.path().join("file-default.sqlite");
    let inherited_db = dir.path().join("inherited.sqlite");
    std::fs::write(
        dir.path().join("taskix.env"),
        format!("TASKIX_DECISION_METRICS_DB={}\n", file_db.display()),
    )
    .unwrap();
    for inherited in [false, true] {
        let mut cmd = Command::new(binary());
        cmd.arg("--config")
            .arg(&config)
            .args(["routing", "metrics", "report", "--json"])
            .env_remove("TASKIX_DECISION_METRICS_DB")
            .env_remove("TASKIX_JEV_METRICS_DB");
        if inherited {
            cmd.env("TASKIX_DECISION_METRICS_DB", &inherited_db);
        }
        let output = cmd.output().unwrap();
        let expected = if inherited { &inherited_db } else { &file_db };
        assert!(!output.status.success());
        assert!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected.to_str().unwrap()),
            "{output:?}"
        );
    }
    assert!(!file_db.exists());
}
