use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub(super) fn prepare_directory(root: &Path, directory: &Path) -> Result<()> {
    ensure!(
        root.join(".obsidian").is_dir(),
        "memory projection vault unavailable"
    );
    let mut current = root.to_owned();
    for part in directory.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "projection directory must not be a symlink or file"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&current)?,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub(super) fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "memory note must be a regular file"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let mut content = String::new();
    File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_string(&mut content)?;
    ensure!(content.len() <= 128 * 1024, "memory note exceeds 128 KiB");
    Ok(Some(content))
}
/// Capture the replaced inode in Recovery before installing a new file without
/// overwriting any concurrently created destination. Recovery copies are retained.
pub(super) fn publish(path: &Path, expected: Option<&str>, text: &str) -> Result<()> {
    ensure!(
        read(path)?.as_deref() == expected,
        "conflict: memory note changed during synchronization"
    );
    let parent = path.parent().context("missing note directory")?;
    let token = uuid::Uuid::now_v7().simple().to_string();
    let temporary = parent.join(format!(".memory-{token}.tmp"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        let mut backup = None;
        if expected.is_some() {
            let recovery = parent.join("Recovery");
            if !recovery.exists() {
                std::fs::create_dir(&recovery)?;
            }
            let meta = std::fs::symlink_metadata(&recovery)?;
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "unsafe recovery directory"
            );
            let saved = recovery.join(format!(
                "{}-{token}.md",
                path.file_stem()
                    .context("missing memory ID")?
                    .to_string_lossy()
            ));
            std::fs::rename(path, &saved)?;
            if read(&saved)?.as_deref() != expected {
                let _ = std::fs::hard_link(&saved, path);
                anyhow::bail!("conflict: concurrent edit preserved in {}", saved.display());
            }
            backup = Some(saved);
        }
        if let Err(error) = std::fs::hard_link(&temporary, path) {
            if let Some(saved) = backup {
                let _ = std::fs::hard_link(saved, path);
            }
            return Err(error.into());
        }
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temporary);
    result
}
