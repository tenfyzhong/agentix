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
    assert!(workflow.contains("${binary}-${RELEASE_TAG}-${TARGET}.tar.gz"));
    assert!(workflow.contains("${binary}-${RELEASE_TAG}-${TARGET}.zip"));
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
fn release_uploads_and_checksums_cover_both_binary_archives() {
    let workflow = repository_file(".github/workflows/release.yml");
    assert!(workflow.contains("--package taskcli"));
    assert!(workflow.contains("name: release-${{ matrix.target }}"));
    assert!(workflow.contains("pattern: release-*"));
    for binary in ["agentix", "taskcli"] {
        for extension in ["tar.gz", "zip"] {
            assert!(workflow.contains(&format!("dist/{binary}-*.{extension}")));
        }
    }
    assert!(workflow.contains("sha256sum agentix-* taskcli-* > SHA256SUMS"));
}

#[cfg(unix)]
mod release_archives {
    use super::*;
    use std::collections::BTreeSet;

    const PLUGIN_FILES: [&str; 12] = [
        ".codex-plugin/plugin.json",
        ".claude-plugin/plugin.json",
        "package.json",
        "package-lock.json",
        "README.md",
        "runtime.mjs",
        "hooks/hooks.json",
        "hooks/run.mjs",
        "extensions/pi.ts",
        "extensions/omp.ts",
        "skills/agent-task-manager/SKILL.md",
        "skills/agent-task-manager/references/commands.md",
    ];

    fn step_script(workflow: &str, name: &str) -> String {
        let step = workflow
            .split(&format!("      - name: {name}\n"))
            .nth(1)
            .unwrap_or_else(|| panic!("missing workflow step: {name}"))
            .split("\n      - name:")
            .next()
            .unwrap();
        let script = step.split_once("        run: ").unwrap().1;
        if let Some(block) = script.strip_prefix("|\n") {
            block
                .lines()
                .map(|line| line.strip_prefix("          ").unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            script.trim().to_owned()
        }
    }

    fn fixture_file(root: &Path, path: &str, body: &[u8]) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn archive_files(path: &Path, tar: &str) -> BTreeSet<String> {
        assert!(path.is_file(), "missing archive: {}", path.display());
        let output = Command::new(tar).arg("-tf").arg(path).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter(|line| !line.ends_with('/'))
            .map(|line| line.trim_start_matches("./").to_owned())
            .collect()
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Release Test",
                "-c",
                "user.email=release@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn prepare_repository(target: &str, suffix: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for path in [
            "README.md",
            "LICENSE",
            "docs/task-board.md",
            "docs/task-workflow-mechanisms.md",
        ] {
            fixture_file(root, path, repository_file(path).as_bytes());
        }
        for binary in ["agentix", "taskcli"] {
            fixture_file(
                root,
                &format!("config/{binary}.example.toml"),
                binary.as_bytes(),
            );
            fixture_file(
                root,
                &format!("target/{target}/release/{binary}{suffix}"),
                binary.as_bytes(),
            );
            for file in [
                format!("{binary}.bash"),
                format!("_{binary}"),
                format!("{binary}.fish"),
            ] {
                let path = format!("completions/{file}");
                fixture_file(root, &path, repository_file(&path).as_bytes());
            }
        }
        for file in PLUGIN_FILES {
            let path = format!("plugins/agent-task-manager/{file}");
            fixture_file(root, &path, repository_file(&path).as_bytes());
        }
        git(root, &["init", "--initial-branch=test/release-packaging"]);
        git(root, &["add", "plugins"]);
        git(
            root,
            &["commit", "-s", "-m", "test: prepare release plugin fixture"],
        );
        fixture_file(
            root,
            "plugins/agent-task-manager/node_modules/untracked.txt",
            b"exclude me",
        );
        directory
    }

    fn expected_archive_files(binary: &str, suffix: &str) -> BTreeSet<String> {
        let mut expected = BTreeSet::from([
            format!("{binary}{suffix}"),
            "README.md".into(),
            "LICENSE".into(),
            format!("{binary}.example.toml"),
            format!("completions/{binary}.bash"),
            format!("completions/_{binary}"),
            format!("completions/{binary}.fish"),
        ]);
        if binary == "taskcli" {
            expected.extend([
                "docs/task-board.md".into(),
                "docs/task-workflow-mechanisms.md".into(),
            ]);
            expected.extend(PLUGIN_FILES.map(|file| format!("plugins/agent-task-manager/{file}")));
        }
        expected
    }

    #[test]
    fn packaging_keeps_binaries_and_resources_separate_and_checksums_both_archives() {
        let workflow = repository_file(".github/workflows/release.yml");
        let packaging = step_script(&workflow, "Package release binary");
        let checksums = step_script(&workflow, "Create checksums");
        for (runner, target) in [
            ("macOS", "aarch64-apple-darwin"),
            ("Linux", "x86_64-unknown-linux-gnu"),
            ("Linux", "aarch64-unknown-linux-gnu"),
            // macOS and Windows both ship libarchive tar with ZIP support.
            #[cfg(target_os = "macos")]
            ("Windows", "x86_64-pc-windows-msvc"),
        ] {
            let suffix = if runner == "Windows" { ".exe" } else { "" };
            let extensions: &[&str] = if runner == "Windows" {
                &["tar.gz", "zip"]
            } else {
                &["tar.gz"]
            };
            let directory = prepare_repository(target, suffix);
            let root = directory.path();
            let tar = if runner == "Windows" { "bsdtar" } else { "tar" };
            let script = if runner == "Windows" {
                format!("tar() {{ bsdtar \"$@\"; }}\n{packaging}")
            } else {
                packaging.clone()
            };
            let output = Command::new("bash")
                .args(["--noprofile", "--norc", "-e", "-o", "pipefail", "-c"])
                .arg(script)
                .env("RUNNER_OS", runner)
                .env("RELEASE_TAG", "1.2.3")
                .env("TARGET", target)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            for binary in ["agentix", "taskcli"] {
                let expected = expected_archive_files(binary, suffix);
                for extension in extensions {
                    let archive = root.join(format!("dist/{binary}-1.2.3-{target}.{extension}"));
                    assert_eq!(
                        archive_files(&archive, tar),
                        expected,
                        "unexpected contents of {}",
                        archive.display()
                    );
                    if *extension == "zip" {
                        assert!(fs::read(archive).unwrap().starts_with(b"PK\x03\x04"));
                    }
                }
            }
            let output = Command::new("bash")
                .args([
                    "--noprofile",
                    "--norc",
                    "-e",
                    "-o",
                    "pipefail",
                    "-c",
                    &checksums,
                ])
                .current_dir(root.join("dist"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let sums = fs::read_to_string(root.join("dist/SHA256SUMS")).unwrap();
            assert_eq!(sums.lines().count(), 2 * extensions.len());
            for binary in ["agentix", "taskcli"] {
                for extension in extensions {
                    assert!(sums.contains(&format!("{binary}-1.2.3-{target}.{extension}")));
                }
            }
            assert!(
                Command::new("sha256sum")
                    .args(["--check", "SHA256SUMS"])
                    .current_dir(root.join("dist"))
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
    }
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
