//! A native fake CLI keeps setup subprocess tests isolated on Unix and Windows.
use std::{path::Path, process::Command, sync::OnceLock};

pub fn install(bin: &Path) {
    static BUILD: OnceLock<tempfile::TempDir> = OnceLock::new();
    let build = BUILD.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fake.rs");
        std::fs::write(&source, include_str!("fake_obsidian.rs")).unwrap();
        let status = Command::new("rustc")
            .arg("--edition=2024")
            .arg(&source)
            .arg("-o")
            .arg(
                dir.path()
                    .join(format!("obsidian{}", std::env::consts::EXE_SUFFIX)),
            )
            .status()
            .unwrap();
        assert!(status.success());
        dir
    });
    let name = format!("obsidian{}", std::env::consts::EXE_SUFFIX);
    std::fs::copy(build.path().join(&name), bin.join(name)).unwrap();
}
