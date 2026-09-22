use std::env;
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo supplies the package version");
    let version = if version.ends_with("-dev") {
        // A deliberately absent input refreshes Git metadata on every Cargo invocation,
        // including untracked files, restored files and commits with unchanged sources.
        let refresh = Path::new(&env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"))
            .join("refresh-git-version");
        println!("cargo:rerun-if-changed={}", refresh.display());
        let manifest = env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies the manifest path");
        let root = Path::new(&manifest).join("../..");
        // Archives must not accidentally inherit metadata from an enclosing repository.
        let metadata = root
            .join(".git")
            .exists()
            .then(|| {
                let hash = git(&root, &["rev-parse", "--short=12", "HEAD"])?;
                let status = git(
                    &root,
                    &["status", "--porcelain=v1", "--untracked-files=normal"],
                )?;
                Some(format!(
                    "{hash}{}",
                    if status.is_empty() { "" } else { ".dirty" }
                ))
            })
            .flatten();
        format!("{version}+{}", metadata.as_deref().unwrap_or("unknown"))
    } else {
        println!("cargo:rerun-if-changed=../../build.rs");
        version
    };
    println!("cargo:rustc-env=AGENTIX_BUILD_VERSION={version}");
}
