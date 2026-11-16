fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_agentix")
}
include!("../../agentix-task/tests/support/env_cli.rs");

#[test]
#[cfg(unix)]
fn launcher_inherits_local_defaults_without_global_exports() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let command = dir.path().join("mock-agent");
    std::fs::write(
        &command,
        "#!/bin/sh\nprintf '%s' \"$LOCAL_ENV_LAUNCH_TEST\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        dir.path().join("agentix.env"),
        "LOCAL_ENV_LAUNCH_TEST=from-file\n",
    )
    .unwrap();
    let output = Command::new(binary())
        .arg("--config")
        .arg(dir.path().join("config.toml"))
        .args(["launch-open-code", "--command"])
        .arg(command)
        .arg("--state-dir")
        .arg(dir.path().join("state"))
        .args(["--endpoint", "tcp://127.0.0.1:12345"])
        .env_remove("LOCAL_ENV_LAUNCH_TEST")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"from-file");
}
