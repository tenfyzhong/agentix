use std::{fs, io::Write, path::Path, time::Duration};

use agentix_task::{Config, DocumentFormat, expand_home};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

const VERSION: &str = "4.12.5";
const PRESET: &str =
    include_str!("../../../plugins/agent-task-manager/obsidian/tasknotes-settings.json");
const FILES: [&str; 3] = ["manifest.json", "main.js", "styles.css"];

pub async fn setup(config: &Config, plugin_dir: Option<&Path>) -> Result<Value> {
    ensure!(
        config.documents.format == DocumentFormat::Obsidian,
        "Obsidian setup requires documents.format = obsidian; run taskcli init first"
    );
    let root = config.documents.root.canonicalize()?.join(".obsidian");
    check_path(&root, "")?;
    let mut changes = Vec::new();
    let (settings, settings_before) = read_json(&root, "plugins/tasknotes/data.json", json!({}))?;
    changes.push(change(
        "plugins/tasknotes/data.json",
        settings_before,
        &merge_settings(settings)?,
    )?);
    let (mut community, community_before) = read_json(&root, "community-plugins.json", json!([]))?;
    enable_array(&mut community, "tasknotes")?;
    changes.push(change(
        "community-plugins.json",
        community_before,
        &community,
    )?);
    let (mut core, core_before) = read_json(&root, "core-plugins.json", json!({}))?;
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

    for name in FILES {
        check_path(&root, &format!("plugins/tasknotes/{name}"))?;
    }
    let installed_manifest = root.join("plugins/tasknotes/manifest.json");
    let already_installed =
        installed_manifest.is_file() && root.join("plugins/tasknotes/main.js").is_file();
    let install = plugin_dir.is_some() || !already_installed;
    let version;
    if install {
        let bundle = if let Some(directory) = plugin_dir {
            let directory = expand_home(directory)?;
            FILES
                .iter()
                .map(|name| {
                    fs::read(directory.join(name)).with_context(|| format!("read TaskNotes {name}"))
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            download(&format!(
                "https://github.com/callumalpass/tasknotes/releases/download/{VERSION}"
            ))
            .await?
        };
        version = validate_bundle(&bundle)?;
        for (name, bytes) in FILES.iter().zip(bundle) {
            changes.push(Change::new(
                &root,
                &format!("plugins/tasknotes/{name}"),
                bytes,
            )?);
        }
    } else {
        version = validate_manifest(&fs::read(installed_manifest)?)?;
        ensure!(
            fs::metadata(root.join("plugins/tasknotes/main.js"))?.len() > 0,
            "TaskNotes main.js is empty; reinstall using --plugin-dir"
        );
    }
    changes.retain(|c| c.before.as_deref() != Some(c.after.as_slice()));
    let modified = !changes.is_empty();
    let backup = apply(&root, &changes)?;
    Ok(
        json!({"vault":config.documents.root,"version":version,"installed":install,"changed":modified,"backup":backup,"restart_required":true,"next_step":"Open or restart Obsidian. If Restricted mode is on, turn it off in Settings > Community plugins to load TaskNotes."}),
    )
}

fn merge_settings(mut settings: Value) -> Result<Value> {
    let object = settings
        .as_object_mut()
        .context("TaskNotes settings must be a JSON object")?;
    let preset: Value = serde_json::from_str(PRESET)?;
    let mut statuses = object.get("customStatuses").cloned().unwrap_or(json!([]));
    let statuses = statuses
        .as_array_mut()
        .context("customStatuses must be an array")?;
    ensure!(
        statuses
            .iter()
            .all(|s| s.is_object() && s["value"].is_string()),
        "invalid custom status definition"
    );
    let mut values = std::collections::HashSet::new();
    ensure!(
        statuses
            .iter()
            .all(|s| values.insert(s["value"].as_str().unwrap_or_default())),
        "duplicate custom status values"
    );
    for wanted in preset["customStatuses"]
        .as_array()
        .context("invalid bundled statuses")?
    {
        // Preserve unrelated status definitions and identity references in other views.
        if let Some(existing) = statuses.iter_mut().find(|s| s["value"] == wanted["value"]) {
            let id = existing.get("id").cloned();
            existing.as_object_mut().context("invalid status")?.extend(
                wanted
                    .as_object()
                    .context("invalid bundled status")?
                    .clone(),
            );
            if let Some(id) = id {
                existing["id"] = id;
            }
        } else {
            statuses.push(wanted.clone());
        }
    }
    let mut ids = std::collections::HashSet::new();
    ensure!(
        statuses.iter().all(|s| s["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && ids.insert(id))),
        "missing or duplicate custom status IDs"
    );
    object.insert("customStatuses".into(), json!(statuses));
    for key in [
        "taskIdentificationMethod",
        "taskTag",
        "openTaskAfterCreation",
        "defaultTaskStatus",
    ] {
        object.insert(key.into(), preset[key].clone());
    }
    let mapping = object
        .entry("fieldMapping")
        .or_insert(json!({}))
        .as_object_mut()
        .context("fieldMapping must be an object")?;
    for key in [
        "title",
        "status",
        "projects",
        "dateCreated",
        "dateModified",
        "completedDate",
    ] {
        mapping.insert(key.into(), json!(key));
    }
    mapping.insert("archiveTag".into(), json!("archived"));
    Ok(settings)
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

fn validate_manifest(bytes: &[u8]) -> Result<String> {
    let manifest: Value = serde_json::from_slice(bytes).context("invalid TaskNotes manifest")?;
    ensure!(
        manifest["id"] == "tasknotes",
        "plugin manifest id must be tasknotes"
    );
    let version = manifest["version"]
        .as_str()
        .context("missing TaskNotes version")?;
    let numbers = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        numbers.len() == 3 && numbers[0] == 4 && (numbers[1], numbers[2]) >= (12, 5),
        "TaskNotes 4.12.5 or newer within major version 4 is required"
    );
    Ok(version.into())
}
fn validate_bundle(bundle: &[Vec<u8>]) -> Result<String> {
    ensure!(
        bundle.len() == 3 && !bundle[1].is_empty(),
        "TaskNotes release is incomplete or main.js is empty"
    );
    validate_manifest(&bundle[0])
}

async fn download(base: &str) -> Result<Vec<Vec<u8>>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_mins(1))
        .user_agent("agentix-taskcli")
        .build()?;
    let mut bundle = Vec::new();
    for name in FILES {
        let mut response = client
            .get(format!("{base}/{name}"))
            .send()
            .await
            .with_context(|| {
                format!("download TaskNotes {name}; use --plugin-dir for an offline release")
            })?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 32 * 1024 * 1024,
                "TaskNotes asset exceeds 32 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        bundle.push(bytes);
    }
    Ok(bundle)
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
        check_path(root, "taskcli-backups")?;
        let parent = root.join("taskcli-backups");
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
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn downloads_release_assets_and_reports_http_failures() {
        for fail in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                for name in FILES {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = [0; 4096];
                    let len = socket.read(&mut request).await.unwrap();
                    assert!(
                        String::from_utf8_lossy(&request[..len])
                            .starts_with(&format!("GET /{name} "))
                    );
                    let status = if fail {
                        "503 Service Unavailable"
                    } else {
                        "200 OK"
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{name}",
                        name.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                    if fail {
                        break;
                    }
                }
            });
            let result = download(&base).await;
            if fail {
                assert!(result.unwrap_err().to_string().contains("503"));
            } else {
                assert_eq!(result.unwrap(), FILES.map(|s| s.as_bytes().to_vec()));
            }
            server.await.unwrap();
        }
    }

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
        assert!(!root.path().join("taskcli-backups").exists());
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
