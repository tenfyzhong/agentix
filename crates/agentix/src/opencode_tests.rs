use super::*;

#[test]
fn opencode_launch_preserves_paths_and_explicit_bridge_environment() {
    let directory = Path::new("/bridge/state with 'quotes'");
    let executable = Path::new("/apps/Open Code/opencode");
    let endpoint = "tcp://127.0.0.1:34123";
    let args = opencode_workspace_args(executable, directory, endpoint);
    let parsed = Cli::try_parse_from(std::iter::once("agentix".to_owned()).chain(args)).unwrap();
    let CliCommand::LaunchOpenCode {
        command,
        state_dir,
        endpoint: actual,
    } = parsed.command
    else {
        panic!("expected the internal OpenCode launcher");
    };
    let child = opencode_launch_command(&command, &state_dir, &actual);
    assert_eq!(child.get_program(), executable.as_os_str());
    let vars = child
        .get_envs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        vars[std::ffi::OsStr::new("AGENTIX_OPENCODE_STATE_DIR")],
        Some(directory.as_os_str())
    );
    assert_eq!(
        vars[std::ffi::OsStr::new("AGENTIX_CONTROL_ENDPOINT")],
        Some(std::ffi::OsStr::new(endpoint))
    );
    assert_eq!(child.get_args().collect::<Vec<_>>(), ["--standalone"]);
}

#[test]
fn opencode_doctor_requires_supported_stable_host_version() {
    for version in ["opencode v2.0.32", "2.0.32\n", "2.1.0", "3.0.0"] {
        assert!(validate_opencode_version(version).is_ok(), "{version}");
    }
    for version in [
        "2.0.31",
        "opencode v2.0.0",
        "1.19.3",
        "opencode v1.19.3",
        "1.19.2",
        "1.18.100",
        "0.99.0",
        "unknown",
        "2.0.32-dev",
        "1.19",
        "1.19.3.4",
    ] {
        assert!(validate_opencode_version(version).is_err(), "{version}");
    }
}

#[test]
fn opencode_doctor_resolves_windows_executable_extensions() {
    let root = tempfile::tempdir().unwrap();
    let command = root.path().join("opencode.EXE");
    std::fs::write(&command, b"fixture").unwrap();
    assert_eq!(
        resolve_executable_in(
            Path::new("opencode"),
            &[root.path().to_owned()],
            &[".COM".into(), ".EXE".into()]
        ),
        Some(command.clone())
    );
    assert_eq!(
        resolve_executable_in(&root.path().join("opencode"), &[], &[".EXE".into()]),
        Some(command)
    );
}

#[test]
fn opencode_launch_resolves_relative_executable_path() {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let file = root.path().join("opencode");
    std::fs::write(&file, b"fixture").unwrap();
    let relative = file.strip_prefix(std::env::current_dir().unwrap()).unwrap();
    let command = opencode_launch_command(relative, root.path(), "tcp://localhost:1234");
    assert_eq!(
        command.get_program(),
        std::fs::canonicalize(file).unwrap().as_os_str()
    );
}

#[cfg(windows)]
#[test]
fn opencode_launch_resolves_cmd_shim() {
    let root = tempfile::tempdir().unwrap();
    let shim = root.path().join("opencode.cmd");
    std::fs::write(&shim, b"@echo 2.0.32\r\n").unwrap();
    let mut command = opencode_launch_command(
        &root.path().join("opencode"),
        root.path(),
        "tcp://localhost:1234",
    );
    assert_eq!(
        command.get_program(),
        std::fs::canonicalize(&shim).unwrap().as_os_str()
    );
    let output = opencode_command(&root.path().join("opencode"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "2.0.32");
    assert!(command.output().unwrap().status.success());
}

#[test]
fn opencode_windows_resolution_prefers_cmd_over_npm_shell_shim() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("opencode"), b"#!/bin/sh").unwrap();
    let shim = root.path().join("opencode.CMD");
    std::fs::write(&shim, b"@echo fixture").unwrap();
    assert_eq!(
        resolve_executable_in(
            Path::new("opencode"),
            &[root.path().to_owned()],
            &[".EXE".into(), ".CMD".into()]
        ),
        Some(shim)
    );
}
