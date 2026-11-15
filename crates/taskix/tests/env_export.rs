fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_taskix")
}
include!("../../agentix-task/tests/support/env_cli.rs");

#[test]
fn env_honors_taskix_config_and_rejects_json() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    std::fs::write(&config, "[environment]\nTASKIX_EXAMPLE = 42\n").unwrap();
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
