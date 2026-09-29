use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

pub struct ReplayState {
    _lock: File,
    directory: PathBuf,
    pub completed: usize,
    pub next_request: usize,
}

impl ReplayState {
    pub fn open(directory: &Path, manifest: &Value, resume: bool) -> Result<Self> {
        if resume {
            ensure!(directory.is_dir(), "resume directory missing");
        } else {
            fs::create_dir(directory).context("run directory already exists or parent missing")?;
        }
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("run.lock"))?;
        lock.try_lock()
            .context("benchmark run already has an owner")?;
        if resume {
            let saved: Value = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
            ensure!(
                saved == *manifest,
                "resume manifest mismatch: inputs, repository, binary or model changed"
            );
        } else {
            atomic_json(directory, "manifest.json", manifest)?;
        }
        let progress = directory.join("progress.json");
        let completed = if progress.exists() {
            serde_json::from_slice(&fs::read(progress)?)?
        } else {
            0
        };
        let mut next_request = 0;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(number) = name
                .strip_prefix("request-")
                .and_then(|n| n.strip_suffix(".input.json"))
            {
                next_request = next_request.max(number.parse::<usize>()? + 1);
            }
        }
        Ok(Self {
            _lock: lock,
            directory: directory.to_owned(),
            completed,
            next_request,
        })
    }

    pub fn checkpoint(&mut self, completed: usize) -> Result<()> {
        ensure!(
            completed >= self.completed,
            "checkpoint cannot move backward"
        );
        atomic_json(
            &self.directory,
            "progress.json",
            &serde_json::json!(completed),
        )?;
        self.completed = completed;
        Ok(())
    }
}

fn atomic_json(directory: &Path, name: &str, value: &Value) -> Result<()> {
    let pending = directory.join(format!("{name}.pending"));
    let mut file = File::create(&pending)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    fs::rename(pending, directory.join(name))?;
    Ok(())
}

pub fn file_digest(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

pub fn repository_digest(root: &Path) -> Result<String> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_name() == ".git" {
                continue;
            }
            let kind = entry.file_type()?;
            ensure!(
                !kind.is_symlink(),
                "benchmark fixture repository must not contain symlinks"
            );
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push((
                    entry
                        .path()
                        .strip_prefix(root)?
                        .to_string_lossy()
                        .into_owned(),
                    file_digest(&entry.path())?,
                ));
            }
        }
    }
    files.sort();
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&files)?)))
}
