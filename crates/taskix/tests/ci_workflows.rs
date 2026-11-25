//! Guard coverage when the CI test matrix is repartitioned.
use std::{collections::BTreeSet, path::Path};

use serde_yaml::Value;

fn workflow(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.github/workflows")
        .join(name);
    serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn step<'a>(job: &'a Value, name: &str) -> &'a Value {
    job["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("missing coverage step: {name}"))
}

#[cfg(unix)]
#[test]
fn linux_tools_retry_the_official_archive_when_apt_update_or_install_fails() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    let action: Value = serde_yaml::from_str(include_str!(
        "../../../.github/actions/install-linux-tools/action.yml"
    ))
    .unwrap();
    let script = action["runs"]["steps"][0]["run"]
        .as_str()
        .unwrap()
        .replace("/etc/os-release", "./os-release");
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("os-release"),
        "ID=ubuntu\nVERSION_CODENAME=noble\n",
    )
    .unwrap();
    let sudo = dir.path().join("sudo");
    fs::write(
        &sudo,
        r#"#!/bin/sh
set -eu
test "$1" = apt-get
shift
sources=
while [ "$1" = -o ]; do
    case "$2" in Dir::Etc::sourcelist=*) sources=${2#*=};; esac
    shift 2
done
uri=$(awk '$1 == "URIs:" {print $2}' "$sources")
case "$uri" in
    https://mirrors.edge.kernel.org/ubuntu) mirror=primary; fail=$FAIL_PRIMARY ;;
    https://archive.ubuntu.com/ubuntu) mirror=official; fail=$FAIL_OFFICIAL ;;
    *) exit 1 ;;
esac
printf '%s %s\n' "$mirror" "$*" >> calls
if [ "$1" = "$fail" ]; then exit 100; fi
"#,
    )
    .unwrap();
    fs::set_permissions(sudo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(dir.path().to_owned())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let fallback = "official update\nofficial install --yes protobuf-compiler fish\n";
    for (primary, official, expected, success) in [
        (
            "install",
            "",
            format!("primary update\nprimary install --yes protobuf-compiler fish\n{fallback}"),
            true,
        ),
        ("update", "", format!("primary update\n{fallback}"), true),
        (
            "install",
            "install",
            format!("primary update\nprimary install --yes protobuf-compiler fish\n{fallback}"),
            false,
        ),
        (
            "",
            "",
            "primary update\nprimary install --yes protobuf-compiler fish\n".to_owned(),
            true,
        ),
    ] {
        let calls = dir.path().join("calls");
        fs::write(&calls, "").unwrap();
        let result = Command::new("bash")
            .args(["--noprofile", "--norc", "-eo", "pipefail", "-c", &script])
            .current_dir(dir.path())
            .env("PATH", &path)
            .env("RUNNER_TEMP", dir.path())
            .env("PACKAGES", "protobuf-compiler fish")
            .env("FAIL_PRIMARY", primary)
            .env("FAIL_OFFICIAL", official)
            .output()
            .unwrap();
        assert_eq!(fs::read_to_string(calls).unwrap(), expected);
        assert_eq!(result.status.success(), success, "{result:?}");
    }
}

#[test]
fn test_matrix_covers_every_partition_once_on_each_platform() {
    let config = workflow("tests.yml");
    let job = &config["jobs"]["test"];
    let entries = job["strategy"]["matrix"]["include"].as_sequence().unwrap();
    for (os, partitions) in [
        ("ubuntu-latest", 2),
        ("macos-latest", 4),
        ("windows-latest", 4),
    ] {
        let platform: Vec<_> = entries
            .iter()
            .filter(|entry| entry["os"].as_str() == Some(os))
            .collect();
        assert_eq!(
            platform.len(),
            partitions,
            "missing or duplicate {os} shard"
        );
        let shards: BTreeSet<_> = platform
            .iter()
            .map(|entry| entry["shard"].as_u64().expect("numeric shard"))
            .collect();
        assert_eq!(shards, (1..=u64::try_from(partitions).unwrap()).collect());
        for entry in platform {
            assert_eq!(entry["shards"].as_u64(), Some(partitions as u64));
        }
    }
    assert_eq!(entries.len(), 10, "unexpected platform or partition");
    assert_eq!(job["strategy"]["fail-fast"].as_bool(), Some(false));
    for name in ["Run workspace tests", "Run Windows tests"] {
        let run = step(job, name)["run"].as_str().unwrap();
        assert!(run.contains("cargo nextest run"));
        assert!(run.contains("--profile ci"));
        assert!(run.contains("--all-features"));
        assert!(run.contains("--no-fail-fast"));
        assert!(run.contains("--partition hash:${{ matrix.shard }}/${{ matrix.shards }}"));
    }
    assert!(
        step(job, "Run workspace tests")["run"]
            .as_str()
            .unwrap()
            .contains("--workspace")
    );
    let windows = step(job, "Run Windows tests")["run"].as_str().unwrap();
    for package in ["agentix-task", "taskix", "agentix-memory"] {
        assert!(windows.contains(&format!("-p {package}")));
    }
}

#[test]
fn ci_profile_prioritizes_the_slow_native_plugin_integration() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.config/nextest.toml");
    let config: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let profile = &config["profile"]["ci"];
    assert_eq!(profile["fail-fast"].as_bool(), Some(false));
    let integration = profile["overrides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| {
            rule["filter"].as_str().is_some_and(|filter| {
                filter.contains("plugin_entrypoints_execute_the_compiled_taskix")
            })
        })
        .expect("slow native integration should run before the ordinary tests");
    assert!(integration["priority"].as_integer().unwrap() > 0);
}

#[test]
fn ci_profile_isolates_memory_ipc_deadline_tests() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.config/nextest.toml");
    let config: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let ipc = config["profile"]["ci"]["overrides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["filter"].as_str() == Some("package(agentix-memory) & binary(ipc)"))
        .expect("real IPC deadlines need the runner without competing tests");
    assert_eq!(ipc["threads-required"].as_str(), Some("num-test-threads"));
    assert!(ipc["priority"].as_integer().unwrap() > 0);
    assert!(ipc.get("retries").is_none());
}

#[test]
fn ci_profile_isolates_the_bounded_memory_metrics_integration() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.config/nextest.toml");
    let config: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let metrics = config["profile"]["ci"]["overrides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| {
            rule["filter"].as_str().is_some_and(|filter| {
                filter.contains("memory_jev_triage_uses_existing_environment_and_reports_skip_extract_and_fallback")
            })
        })
        .expect("the bounded metrics writer needs the runner without competing tests");
    assert_eq!(
        metrics["threads-required"].as_str(),
        Some("num-test-threads")
    );
    assert!(metrics["priority"].as_integer().unwrap() > 0);
    assert!(metrics.get("retries").is_none());
}

#[test]
fn partitioning_preserves_doctests_and_windows_platform_checks() {
    let config = workflow("tests.yml");
    let job = &config["jobs"]["test"];
    assert!(
        step(job, "Run workspace doctests")["if"]
            .as_str()
            .unwrap()
            .contains("matrix.shard == 1")
    );
    assert_eq!(
        step(job, "Run workspace doctests")["run"].as_str(),
        Some("cargo test --workspace --all-features --doc")
    );
    assert_eq!(
        config["jobs"]["windows-platform"]["runs-on"].as_str(),
        Some("windows-latest")
    );
    let job = &config["jobs"]["windows-platform"];
    assert_eq!(
        job["needs"].as_str(),
        Some("classify"),
        "platform checks should wait only for classification, not test partitions"
    );
    assert_eq!(
        config["jobs"]["test"]["needs"].as_str(),
        Some("classify"),
        "test partitions and platform checks should start together"
    );
    assert_eq!(
        step(job, "Check the Windows workspace")["run"].as_str(),
        Some("cargo check --workspace --all-features")
    );
    assert_eq!(
        step(job, "Run Windows TCP control tests")["run"].as_str(),
        Some(
            "cargo test -p agentix --all-features --bin agentix control::tests::tcp_control_server"
        )
    );
    assert_eq!(
        step(job, "Run Windows doctests")["run"].as_str(),
        Some("cargo test -p agentix-memory -p agentix-task -p taskix --all-features --doc")
    );
    let zones = step(job, "Verify Windows system time zones")["run"]
        .as_str()
        .unwrap();
    for name in ["Tokyo Standard Time", "SA Pacific Standard Time", "UTC"] {
        assert!(zones.contains(name));
    }
    assert!(zones.contains("finally"));
    assert!(zones.contains("tzutil /s $originalTimeZone"));
    assert!(zones.contains("task_note_timestamps_follow_the_system_local_zone -- --exact"));
    let steps = job["steps"].as_sequence().unwrap();
    let index = |name| {
        steps
            .iter()
            .position(|step| step["name"].as_str() == Some(name))
            .unwrap()
    };
    assert!(index("Run Windows TCP control tests") < index("Verify Windows system time zones"));
}

#[test]
fn both_ci_workflows_cancel_superseded_runs() {
    for name in ["ci.yml", "tests.yml"] {
        let config = workflow(name);
        assert_eq!(
            config["concurrency"]["cancel-in-progress"].as_bool(),
            Some(true)
        );
        let group = config["concurrency"]["group"].as_str().unwrap();
        assert!(group.contains("github.workflow"));
        assert!(group.contains("github.ref"));
    }
}
