// Homebrew linking uses Unix executable permissions and symlinks.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::{Command, Output};

fn script(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Makefile"),
        root.join("Makefile"),
    )
    .unwrap();
    fs::create_dir_all(root.join("prefix with spaces/bin")).unwrap();
    script(
        &root.join("brew"),
        r#"#!/bin/sh
set -eu
case "$1" in
  --prefix) printf '%s\n' "$PWD/prefix with spaces" ;;
  unlink)
    shift
    for binary in "$@"; do
      printf '%s\n' "$binary" >> unlinked
      rm -f "prefix with spaces/bin/$binary"
    done ;;
  *) exit 2 ;;
esac
"#,
    );
    script(
        &root.join("cargo"),
        r#"#!/bin/sh
set -eu
[ ! -e fail-build ] || exit 1
while [ "$#" -gt 0 ]; do
  if [ "$1" = --target-dir ]; then shift; target_dir=$1; fi
  shift
done
mkdir -p "$target_dir/debug"
for binary in agentix taskix; do
  printf '#!/bin/sh\nexit 0\n' > "$target_dir/debug/$binary"
  chmod +x "$target_dir/debug/$binary"
done
"#,
    );
    for binary in ["agentix", "taskix"] {
        symlink(
            "/old/homebrew/binary",
            root.join("prefix with spaces/bin").join(binary),
        )
        .unwrap();
    }
    dir
}

fn run(root: &Path) -> Output {
    Command::new("make")
        .current_dir(root)
        .args(["link-debug", "BREW=./brew", "CARGO=./cargo"])
        .env("CARGO_TARGET_DIR", "build with spaces")
        .output()
        .unwrap()
}

#[test]
fn links_debug_binaries_and_can_be_repeated() {
    let dir = fixture();
    let root = dir.path();
    for _ in 0..2 {
        let result = run(root);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for binary in ["agentix", "taskix"] {
            let link = root.join("prefix with spaces/bin").join(binary);
            let destination = fs::read_link(&link).unwrap();
            assert!(destination.is_absolute());
            assert_eq!(
                fs::canonicalize(link).unwrap(),
                fs::canonicalize(root.join("build with spaces/debug").join(binary)).unwrap()
            );
        }
    }
    assert_eq!(
        fs::read_to_string(root.join("unlinked")).unwrap(),
        "agentix\ntaskix\nagentix\ntaskix\n"
    );
}

#[test]
fn build_failure_keeps_homebrew_links() {
    let dir = fixture();
    let root = dir.path();
    fs::write(root.join("fail-build"), "").unwrap();
    assert!(!run(root).status.success());
    assert!(!root.join("unlinked").exists());
    for binary in ["agentix", "taskix"] {
        assert_eq!(
            fs::read_link(root.join("prefix with spaces/bin").join(binary)).unwrap(),
            Path::new("/old/homebrew/binary")
        );
    }
}
