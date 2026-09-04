use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn repository_file(path: &str) -> String {
    std::fs::read_to_string(repository_root().join(path))
        .unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}

#[test]
fn release_workflow_builds_tag_aligned_native_archives() {
    let workflow = repository_file(".github/workflows/release.yml");

    assert!(workflow.contains("push:\n    tags:\n      - \"v*\""));
    assert!(workflow.contains("workflow_dispatch:"));
    assert!(workflow.contains(".github/scripts/verify-release-version.sh"));
    assert!(workflow.contains("agentix $VERSION"));
    for target in [
        "aarch64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-pc-windows-msvc",
    ] {
        assert!(workflow.contains(target), "missing release target {target}");
    }
    assert!(workflow.contains("agentix-${RELEASE_TAG}-${TARGET}.tar.gz"));
    assert!(workflow.contains("agentix-${RELEASE_TAG}-${TARGET}.zip"));
    assert!(workflow.contains("gh release create"));
    assert!(workflow.contains("gh release upload"));
    assert!(workflow.contains("TODO(homebrew): Enable after the tap is configured"));
    assert!(!workflow.lines().any(|line| line == "  publish-homebrew:"));
}

#[test]
fn release_version_verifier_requires_the_tag_to_match_cargo() {
    let script = repository_root().join(".github/scripts/verify-release-version.sh");
    let expected = env!("CARGO_PKG_VERSION");
    let accepted = Command::new(&script)
        .arg(format!("v{expected}"))
        .current_dir(repository_root())
        .output()
        .unwrap();
    assert!(accepted.status.success());
    assert_eq!(String::from_utf8(accepted.stdout).unwrap().trim(), expected);

    let rejected = Command::new(script)
        .arg("v999.0.0")
        .current_dir(repository_root())
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8(rejected.stderr)
            .unwrap()
            .contains("does not match workspace version")
    );
}

#[test]
fn homebrew_workflow_is_manual_until_the_release_call_is_enabled() {
    let workflow = repository_file(".github/workflows/homebrew.yml");
    let formula = repository_file("packaging/homebrew/agentix.rb");

    assert!(workflow.contains("workflow_call:"));
    assert!(workflow.contains("workflow_dispatch:"));
    assert!(!workflow.contains("HOMEBREW_PUBLISH_ENABLED"));
    assert!(workflow.contains("HOMEBREW_TAP_TOKEN"));
    assert!(workflow.contains("Formula/agentix.rb"));
    assert!(workflow.contains("brew install --build-bottle"));
    assert!(workflow.contains("brew bottle --json --no-rebuild"));
    assert!(workflow.contains("gh release upload"));
    assert!(workflow.contains("peter-evans/create-pull-request@v7"));
    assert!(formula.contains("class Agentix < Formula"));
    assert!(formula.contains("depends_on \"protobuf\" => :build"));
    assert!(formula.contains("depends_on \"rust\" => :build"));
    assert!(formula.contains("std_cargo_args(path: \"crates/agentix\")"));
    assert!(formula.contains("assert_match version.to_s"));
}
