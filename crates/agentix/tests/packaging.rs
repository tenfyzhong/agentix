use std::fs;
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

    assert!(workflow.contains("push:\n    tags:\n      - \"[0-9]+.[0-9]+.[0-9]+\""));
    assert!(workflow.contains("workflow_dispatch:"));
    assert!(workflow.contains(".github/scripts/release-version.sh"));
    assert!(workflow.contains(".github/scripts/set-release-version.sh"));
    assert!(workflow.contains("Set release version"));
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
    assert!(workflow.lines().any(|line| line == "  publish-homebrew:"));
    assert!(workflow.contains("uses: ./.github/workflows/homebrew.yml"));
    assert!(workflow.contains("HOMEBREW_TAP_TOKEN: ${{ secrets.HOMEBREW_TAP_TOKEN }}"));
}

#[test]
fn workspace_uses_a_development_version_until_release_packaging() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.0.0-dev");
}

#[test]
fn release_and_homebrew_include_standalone_taskcli_and_host_plugin() {
    let workflow = repository_file(".github/workflows/release.yml");
    assert!(workflow.contains("--package taskcli"));
    assert!(workflow.contains("release/taskcli.exe"));
    assert!(workflow.contains("release/taskcli\""));
    assert!(workflow.contains("plugins/agent-task-manager"));
    let formula = repository_file("packaging/homebrew/agentix.rb");
    assert!(formula.contains("crates/taskcli"));
    assert!(formula.contains("taskcli --version"));
    assert!(formula.contains("plugins/agent-task-manager"));
}

#[test]
fn release_version_setter_updates_workspace_packages_from_the_tag() {
    let temporary_repository = tempfile::tempdir().unwrap();
    let manifest = temporary_repository.path().join("Cargo.toml");
    let lockfile = temporary_repository.path().join("Cargo.lock");
    let crate_directory = temporary_repository.path().join("crates/agentix");
    fs::create_dir_all(&crate_directory).unwrap();
    fs::write(
        &manifest,
        "[workspace]\nmembers = [\"crates/agentix\"]\n\n[workspace.package]\nversion = \"0.0.0-dev\"\n",
    )
    .unwrap();
    fs::write(
        crate_directory.join("Cargo.toml"),
        "[package]\nname = \"agentix\"\nversion.workspace = true\n",
    )
    .unwrap();
    fs::write(
        &lockfile,
        "version = 4\n\n[[package]]\nname = \"agentix\"\nversion = \"0.0.0-dev\"\n\n[[package]]\nname = \"unrelated\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let script = repository_root().join(".github/scripts/set-release-version.sh");
    let rejected = Command::new(&script)
        .args([
            "not-a-version",
            manifest.to_str().unwrap(),
            lockfile.to_str().unwrap(),
        ])
        .current_dir(repository_root())
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        fs::read_to_string(&manifest)
            .unwrap()
            .contains("version = \"0.0.0-dev\"")
    );

    let prepared = Command::new(script)
        .args([
            "v1.2.3",
            manifest.to_str().unwrap(),
            lockfile.to_str().unwrap(),
        ])
        .current_dir(repository_root())
        .output()
        .unwrap();
    assert!(prepared.status.success());
    assert_eq!(String::from_utf8(prepared.stdout).unwrap().trim(), "1.2.3");
    assert!(
        fs::read_to_string(manifest)
            .unwrap()
            .contains("version = \"1.2.3\"")
    );
    let updated_lockfile = fs::read_to_string(lockfile).unwrap();
    assert!(updated_lockfile.contains("name = \"agentix\"\nversion = \"1.2.3\""));
    assert!(
        updated_lockfile.contains("name = \"unrelated\"\nversion = \"0.1.0\""),
        "unrelated packages must not be changed"
    );
}

#[test]
fn homebrew_workflow_builds_a_bottle_and_updates_the_tap() {
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
    assert!(workflow.contains(".github/scripts/release-version.sh"));
    assert!(formula.contains(".github/scripts/set-release-version.sh"));
    assert!(formula.contains("class Agentix < Formula"));
    assert!(formula.contains("depends_on \"protobuf\" => :build"));
    assert!(formula.contains("depends_on \"rust\" => :build"));
    assert!(formula.contains("std_cargo_args(path: \"crates/agentix\")"));
    assert!(formula.contains("assert_match version.to_s"));
}

#[test]
fn readme_documents_installation_on_all_supported_platforms() {
    let readme = repository_file("README.md");

    assert!(readme.contains("macOS and Linux"));
    assert!(readme.contains("brew tap tenfyzhong/tap"));
    assert!(readme.contains("brew install agentix"));
    assert!(readme.contains("brew services start tenfyzhong/tap/agentix"));
    assert!(readme.contains("### Windows (x86_64)"));
    assert!(readme.contains("agentix-<version>-x86_64-pc-windows-msvc.zip"));
    assert!(readme.contains("New-Item -ItemType Directory -Force \"$HOME\\.config\\agentix\""));
    assert!(readme.contains("agentix.exe doctor"));
    assert!(readme.contains("Codex backend is not available on Windows"));
}
