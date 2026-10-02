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

#[test]
fn test_matrix_covers_every_partition_once_on_each_platform() {
    let config = workflow("tests.yml");
    let job = &config["jobs"]["test"];
    let entries = job["strategy"]["matrix"]["include"].as_sequence().unwrap();
    for (os, partitions) in [
        ("ubuntu-latest", 2),
        ("macos-latest", 2),
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
    assert_eq!(entries.len(), 8, "unexpected platform or partition");
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
    assert!(
        job.get("needs").is_none(),
        "platform checks should overlap the test partitions"
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
