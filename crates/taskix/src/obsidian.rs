use std::{fs, io::Write, path::Path, time::Duration};

use agentix_task::Config;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub async fn setup(config: &Config, config_path: &Path, no_reload: bool) -> Result<Value> {
    let root = config.documents.root.canonicalize()?.join(".obsidian");
    check_path(&root, "")?;
    let changes = configuration_changes(&root, config_path)?;

    let installed = !root.join("plugins/taskix-sync/main.js").is_file();
    let modified = changes
        .iter()
        .any(|c| c.before.as_deref() != Some(c.after.as_slice()));
    let mut reload_error = None;
    let mut connection = None;
    if modified && !no_reload {
        match ObsidianCli::connect(root.parent().context("missing vault")?).await {
            Ok(mut cli) => {
                if let Err(error) = cli.suspend().await {
                    reload_error = Some(format!("{error:#}"));
                }
                connection = Some(cli);
            }
            Err(error) => reload_error = Some(format!("{error:#}")),
        }
    }
    // Starting Obsidian and disabling plugins can save configuration. Merge
    // the latest settings and desired enabled list before publication.
    let publication = publish(&root, config_path, changes, connection.is_some());
    let backup = match publication {
        Ok(backup) => backup,
        Err(error) => {
            if let Some(cli) = &connection {
                cli.restore()
                    .await
                    .context(format!("setup failed: {error:#}; plugin recovery failed"))?;
            }
            return Err(error);
        }
    };
    // Re-enabling against the app's old in-memory list after publication can
    // overwrite the installed enabled list. Leave it for the manual restart
    // when reload fails; only restore plugins after a failed installation.
    let reloaded = if let Some(cli) = &connection
        && reload_error.is_none()
    {
        match cli.run(&["reload"]).await {
            Ok(_) => true,
            Err(error) => {
                reload_error = Some(format!("{error:#}"));
                false
            }
        }
    } else {
        false
    };
    let next_step = if reloaded {
        "Obsidian reload requested. If Restricted mode is on, turn it off in Settings > Community plugins to load Taskix Sync. Task boards require Obsidian 1.14 or newer with Bases enabled."
    } else {
        "Open or restart Obsidian. If Restricted mode is on, turn it off in Settings > Community plugins to load Taskix Sync. Task boards require Obsidian 1.14 or newer with Bases enabled."
    };
    Ok(
        json!({"vault":config.documents.root,"min_obsidian_version":"1.14.0","installed":installed,"sync_plugin":"taskix-sync","changed":modified,"backup":backup,"reloaded":reloaded,"reload_error":reload_error,"restart_required":!reloaded,"next_step":next_step}),
    )
}

fn publish(
    root: &Path,
    config_path: &Path,
    mut changes: Vec<Change>,
    refresh_configuration: bool,
) -> Result<Option<std::path::PathBuf>> {
    if refresh_configuration {
        changes.clear();
        changes.extend(configuration_changes(root, config_path)?);
    }
    changes.retain(|c| c.before.as_deref() != Some(c.after.as_slice()));
    apply(root, &changes)
}

fn configuration_changes(root: &Path, config_path: &Path) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    sync_plugin(root, config_path, &mut changes)?;
    let (mut community, community_before) = read_json(root, "community-plugins.json", json!([]))?;
    // Keep installed files and personal settings; remove the obsolete dependency
    // from this vault's enabled plugins without installing or loading TaskNotes.
    if let Some(plugins) = community.as_array_mut() {
        plugins.retain(|id| id != "tasknotes");
    }
    enable_array(&mut community, "taskix-sync")?;
    changes.push(change(
        "community-plugins.json",
        community_before,
        &community,
    )?);
    let (mut core, core_before) = read_json(root, "core-plugins.json", json!({}))?;
    if let Some(object) = core.as_object_mut() {
        ensure!(
            object.values().all(Value::is_boolean),
            "invalid core plugin settings"
        );
        object.insert("bases".into(), json!(true));
    } else {
        enable_array(&mut core, "bases")?;
    }
    changes.push(change("core-plugins.json", core_before, &core)?);

    Ok(changes)
}

/// Use an explicit vault selector and verify its path before any mutations.
/// A same-named vault or an old CLI that ignores targeting must not reload the
/// user's currently focused, unrelated vault.
struct ObsidianCli {
    vault: std::path::PathBuf,
    selector: String,
    suspended: Vec<String>,
}

impl ObsidianCli {
    async fn connect(vault: &Path) -> Result<Self> {
        let name = vault
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid vault name")?;
        let cli = Self {
            vault: vault.to_owned(),
            selector: format!("vault={name}"),
            suspended: Vec::new(),
        };
        let reported = cli.run(&["vault", "info=path"]).await?;
        ensure!(
            Path::new(reported.trim()).canonicalize().ok().as_deref() == Some(vault),
            "Obsidian CLI selected a different or unavailable vault"
        );
        Ok(cli)
    }

    async fn run(&self, args: &[&str]) -> Result<String> {
        let output = tokio::time::timeout(Duration::from_secs(10), tokio::process::Command::new("obsidian")
            .current_dir(&self.vault)
            .arg(&self.selector)
            .args(args)
            .kill_on_drop(true)
            .output()).await.context("Obsidian CLI timed out after 10 seconds")?
            .context("could not run Obsidian CLI; enable it in Settings > General and ensure obsidian is on PATH")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        ensure!(
            output.status.success()
                && !stdout
                    .lines()
                    .chain(stderr.lines())
                    .any(|line| line.trim_start().to_ascii_lowercase().starts_with("error:")),
            "Obsidian CLI {} failed: {} {}",
            args[0],
            stdout.trim(),
            stderr.trim()
        );
        Ok(stdout.into_owned())
    }

    async fn suspend(&mut self) -> Result<()> {
        let enabled: Value = serde_json::from_str(
            &self
                .run(&["plugins:enabled", "filter=community", "format=json"])
                .await?,
        )
        .context("invalid Obsidian enabled plugin response")?;
        let enabled = enabled
            .as_array()
            .context("invalid Obsidian enabled plugin list")?;
        // Record attempted disables too: a CLI timeout can arrive after the
        // app already disabled the plugin. Stop TaskNotes during migration.
        for id in ["taskix-sync", "tasknotes"] {
            if enabled.iter().any(|plugin| plugin["id"] == id) {
                self.suspended.push(id.into());
                self.run(&["plugin:disable", &format!("id={id}"), "filter=community"])
                    .await?;
            }
        }
        Ok(())
    }

    async fn restore(&self) -> Result<()> {
        let mut failures = Vec::new();
        for id in self.suspended.iter().rev() {
            if let Err(error) = self
                .run(&["plugin:enable", &format!("id={id}"), "filter=community"])
                .await
            {
                failures.push(format!("{id}: {error:#}"));
            }
        }
        ensure!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }
}

fn sync_plugin(root: &Path, config_path: &Path, changes: &mut Vec<Change>) -> Result<()> {
    for (name, source) in [
        (
            "main.js",
            include_str!("../../../plugins/taskix-manager/obsidian/taskix-sync/main.js"),
        ),
        (
            "manifest.json",
            include_str!("../../../plugins/taskix-manager/obsidian/taskix-sync/manifest.json"),
        ),
        (
            "styles.css",
            include_str!("../../../plugins/taskix-manager/obsidian/taskix-sync/styles.css"),
        ),
    ] {
        changes.push(Change::new(
            root,
            &format!("plugins/taskix-sync/{name}"),
            source.as_bytes().to_vec(),
        )?);
    }
    let relative = "plugins/taskix-sync/data.json";
    let (mut settings, before) = read_json(root, relative, json!({}))?;
    let object = settings
        .as_object_mut()
        .context("Taskix Sync settings must be a JSON object")?;
    object
        .entry("cliPath")
        .or_insert(json!(std::env::current_exe()?.canonicalize()?));
    object
        .entry("configPath")
        .or_insert(json!(config_path.canonicalize()?));
    changes.push(change(relative, before, &settings)?);
    Ok(())
}

fn enable_array(value: &mut Value, id: &str) -> Result<()> {
    let array = value
        .as_array_mut()
        .context("plugin list must be a JSON array")?;
    ensure!(
        array.iter().all(Value::is_string),
        "plugin list entries must be strings"
    );
    if !array.iter().any(|v| v == id) {
        array.push(json!(id));
    }
    Ok(())
}

// Reject symlinks, including broken links, at every writable path component.
fn check_path(root: &Path, relative: &str) -> Result<()> {
    let mut path = root.to_owned();
    let mut paths = vec![path.clone()];
    for part in Path::new(relative) {
        path.push(part);
        paths.push(path.clone());
    }
    for path in paths {
        match fs::symlink_metadata(&path) {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink(),
                "refusing symlink: {}",
                path.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn read_json(root: &Path, relative: &str, default: Value) -> Result<(Value, Option<Vec<u8>>)> {
    check_path(root, relative)?;
    match fs::read(root.join(relative)) {
        Ok(bytes) => Ok((
            serde_json::from_slice(&bytes).with_context(|| format!("parse {relative}"))?,
            Some(bytes),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((default, None)),
        Err(e) => Err(e.into()),
    }
}
struct Change {
    relative: String,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}
impl Change {
    fn new(root: &Path, relative: &str, after: Vec<u8>) -> Result<Self> {
        check_path(root, relative)?;
        let before = match fs::read(root.join(relative)) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            relative: relative.into(),
            before,
            after,
        })
    }
}
fn change(relative: &str, before: Option<Vec<u8>>, value: &Value) -> Result<Change> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(Change {
        relative: relative.into(),
        before,
        after: bytes,
    })
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
fn apply(root: &Path, changes: &[Change]) -> Result<Option<std::path::PathBuf>> {
    if changes.is_empty() {
        return Ok(None);
    }
    // Check the complete write set before publishing any files.
    for c in changes {
        check_path(root, &c.relative)?;
        ensure!(
            fs::read(root.join(&c.relative)).ok() == c.before,
            "configuration changed during setup; close Obsidian and retry"
        );
    }
    let backup = if changes.iter().any(|c| c.before.is_some()) {
        check_path(root, "taskix-backups")?;
        let parent = root.join("taskix-backups");
        fs::create_dir_all(&parent)?;
        let backup = tempfile::Builder::new()
            .prefix("setup-")
            .tempdir_in(parent)?;
        for c in changes {
            if let Some(bytes) = &c.before {
                atomic_write(&backup.path().join(&c.relative), bytes)?;
            }
        }
        Some(backup.keep())
    } else {
        None
    };
    for (index, c) in changes.iter().enumerate() {
        if let Err(error) = atomic_write(&root.join(&c.relative), &c.after) {
            let mut failures = Vec::new();
            for previous in changes[..index].iter().rev() {
                let path = root.join(&previous.relative);
                let rollback = match &previous.before {
                    Some(bytes) => atomic_write(&path, bytes),
                    None => fs::remove_file(&path).map_err(Into::into),
                };
                if let Err(e) = rollback {
                    failures.push(e.to_string());
                }
            }
            bail!("setup failed: {error}; rollback errors: {failures:?}; backup: {backup:?}");
        }
    }
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_configuration_changes_abort_before_writes() {
        let root = tempfile::tempdir().unwrap();
        let c = Change::new(root.path(), "data.json", b"new".to_vec()).unwrap();
        fs::write(root.path().join("data.json"), "authored concurrently").unwrap();
        assert!(
            apply(root.path(), &[c])
                .unwrap_err()
                .to_string()
                .contains("changed during setup")
        );
        assert_eq!(
            fs::read_to_string(root.path().join("data.json")).unwrap(),
            "authored concurrently"
        );
        assert!(!root.path().join("taskix-backups").exists());
    }

    #[test]
    fn publication_failure_restores_original_configuration() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("existing.json"), "original").unwrap();
        let first = Change::new(root.path(), "existing.json", b"updated".to_vec()).unwrap();
        let second = Change::new(root.path(), "blocked/data.json", b"new".to_vec()).unwrap();
        fs::write(
            root.path().join("blocked"),
            "file prevents directory creation",
        )
        .unwrap();
        assert!(apply(root.path(), &[first, second]).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("existing.json")).unwrap(),
            "original"
        );
    }
}
