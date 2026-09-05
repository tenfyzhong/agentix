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
fn feishu_sdk_uses_a_published_registry_release() {
    let lockfile: toml::Value = toml::from_str(&repository_file("Cargo.lock")).unwrap();
    let sdk = lockfile["package"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"].as_str() == Some("larksuite-oapi-sdk-rs"))
        .unwrap();

    assert_eq!(
        sdk.get("source").and_then(toml::Value::as_str),
        Some("registry+https://github.com/rust-lang/crates.io-index"),
        "the Feishu SDK must use the published crate without local or Git patches"
    );
    assert!(sdk.get("checksum").and_then(toml::Value::as_str).is_some());
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
fn release_archives_include_standalone_taskcli_and_host_plugin() {
    let workflow = repository_file(".github/workflows/release.yml");
    assert!(workflow.contains("--package taskcli"));
    assert!(workflow.contains("release/taskcli.exe"));
    assert!(workflow.contains("release/taskcli\""));
    assert!(workflow.contains("plugins/agent-task-manager"));
    assert!(workflow.contains("config/taskcli.example.toml"));
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

    assert!(
        !repository_root()
            .join("packaging/homebrew/agentix.rb")
            .exists(),
        "the formula must be maintained exclusively in the Homebrew tap"
    );
    assert!(!workflow.contains("packaging/homebrew"));
    assert!(workflow.contains("repository: tenfyzhong/homebrew-tap"));

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
}

#[test]
#[cfg(target_os = "macos")]
fn homebrew_source_update_preserves_tap_customizations_and_removes_old_bottles() {
    let workflow = repository_file(".github/workflows/homebrew.yml");
    let ruby = workflow
        .split("ruby <<'RUBY'\n")
        .nth(1)
        .unwrap()
        .split("          RUBY")
        .next()
        .unwrap();
    let temporary_tap = tempfile::tempdir().unwrap();
    let formula_path = temporary_tap.path().join("agentix.rb");
    let formula = concat!(
        "class Agentix < Formula\n",
        "  desc \"Customized in the tap\"\n",
        "  url \"https://example.com/v1.0.0.tar.gz\"\n",
        "  sha256 \"old-source-checksum\"\n",
        "\n",
        "  bottle do\n",
        "    root_url \"https://example.com/v1.0.0\"\n",
        "    sha256 cellar: :any_skip_relocation, arm64_sequoia: \"old-bottle-checksum\"\n",
        "  end\n",
        "\n",
        "  depends_on \"custom-build-dependency\" => :build\n",
        "  def install\n",
        "    system \"bash\", \".github/scripts/set-release-version.sh\", version.to_s\n",
        "  end\n",
        "  service do\n",
        "    run [opt_bin/\"agentix\", \"serve\"]\n",
        "  end\n",
        "end\n",
    );
    fs::write(&formula_path, formula).unwrap();
    for _ in 0..2 {
        let result = Command::new("ruby")
            .args(["-e", ruby])
            .env("FORMULA_PATH", &formula_path)
            .env("SOURCE_URL", "https://example.com/v2.0.0.tar.gz")
            .env("SOURCE_SHA256", "new-source-checksum")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let updated = fs::read_to_string(&formula_path).unwrap();
        assert!(!updated.contains("bottle do"));
        assert!(!updated.contains("old-bottle-checksum"));
        let expected = formula
            .replace("v1.0.0.tar.gz", "v2.0.0.tar.gz")
            .replace("old-source-checksum", "new-source-checksum")
            .replace(
                concat!(
                    "  bottle do\n",
                    "    root_url \"https://example.com/v1.0.0\"\n",
                    "    sha256 cellar: :any_skip_relocation, arm64_sequoia: \"old-bottle-checksum\"\n",
                    "  end\n\n",
                ),
                "",
            );
        assert_eq!(updated, expected);
    }
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
