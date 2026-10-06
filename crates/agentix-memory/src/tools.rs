use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{MemoryStore, ToolDefinition, ToolInputError, ToolSet};

pub struct ProjectTools {
    store: MemoryStore,
    project: String,
    root: Option<PathBuf>,
    inspections: Mutex<Vec<Value>>,
    checked: AtomicBool,
    memory_checked: AtomicBool,
}

impl ProjectTools {
    pub fn new(store: MemoryStore, project: String, root: Option<PathBuf>) -> Result<Self> {
        ensure!(!project.is_empty(), "missing Project");
        let root = root.map(|p| p.canonicalize()).transpose()?;
        if let Some(root) = &root {
            ensure!(root.is_dir(), "repository root must be a directory");
        }
        Ok(Self {
            store,
            project,
            root,
            inspections: Mutex::new(Vec::new()),
            checked: AtomicBool::new(false),
            memory_checked: AtomicBool::new(false),
        })
    }

    pub fn repository_checked(&self) -> bool {
        self.checked.load(Ordering::Relaxed)
    }

    pub fn memory_checked(&self) -> bool {
        self.memory_checked.load(Ordering::Relaxed)
    }

    pub(crate) async fn related_memories(&self, query: &str) -> Result<Vec<crate::Memory>> {
        let matches = self
            .store
            .search_scoped(&self.project, query, 8, true)
            .await?;
        self.memory_checked.store(true, Ordering::Relaxed);
        Ok(matches)
    }

    pub fn inspection_audit(&self) -> Vec<Value> {
        self.inspections
            .lock()
            .expect("inspection audit lock")
            .clone()
    }

    pub async fn repository_head(&self) -> Result<Option<String>> {
        let root = self.root()?;
        if !root.join(".git").exists() {
            return Ok(None);
        }
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["rev-parse", "--verify", "HEAD"])
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8(output.stdout)?.trim().to_owned()))
    }

    fn record_inspection(&self, name: &str, result: &Value) {
        self.inspections.lock().expect("inspection audit lock").push(json!({"tool":name,"digest":crate::retrieval::digest(&result.to_string()),"path":result.get("path"),"query":result.get("query"),"complete":result.get("complete")}));
    }

    fn root(&self) -> Result<PathBuf> {
        self.root.clone().context("repository unavailable")
    }
}

pub(crate) fn definition(name: &str, description: &str, properties: &Value) -> ToolDefinition {
    let required: Vec<_> = properties
        .as_object()
        .expect("tool properties")
        .keys()
        .cloned()
        .collect();
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFile {
    path: String,
    offset: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadSource {
    receipt_id: String,
    message_id: Option<String>,
    offset: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceAnchor {
    receipt_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Show {
    id: String,
}

#[async_trait]
impl ToolSet for ProjectTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            definition(
                "memory_search",
                "Search active, unexpired Project memories; inspect memory_conflicts separately",
                &json!({"query":{"type":"string"}}),
            ),
            definition(
                "memory_conflicts",
                "Inspect a bounded page of unresolved Project conflicts; they are not effective facts",
                &json!({"after":{"type":"string"}}),
            ),
            definition(
                "memory_show",
                "Read a current Project memory with evidence and revision",
                &json!({"id":{"type":"string"}}),
            ),
            definition(
                "source_neighbors",
                "Discover up to eight preceding receipts in the same Project and session. Use next_receipt_id as the anchor to page farther back; source_read lists and reads their messages.",
                &json!({"receipt_id":{"type":"string"}}),
            ),
            definition(
                "source_read",
                "Read original evidence. Null message_id lists 32 message IDs with a message-index offset; otherwise offset is a UTF-8 byte offset into that message.",
                &json!({"receipt_id":{"type":"string"},"message_id":{"type":["string","null"]},"offset":{"type":"integer","minimum":0}}),
            ),
            definition(
                "repo_read",
                "Read a bounded repository file; offsets are UTF-8 byte offsets",
                &json!({"path":{"type":"string"},"offset":{"type":"integer","minimum":0}}),
            ),
            definition(
                "repo_search",
                "Bounded literal repository search. Query must be nonempty and at most 512 UTF-8 bytes; empty queries cannot list files. Inspect incomplete coverage before concluding absence.",
                &json!({"query":{"type":"string","minLength":1,"maxLength":512}}),
            ),
        ]
    }

    async fn execute(&self, name: &str, arguments: Value) -> Result<Value> {
        match name {
            "memory_search" => {
                let args: Query = serde_json::from_value(arguments)?;
                let matches = self.store.search(&self.project, &args.query, 8).await?;
                self.memory_checked.store(true, Ordering::Relaxed);
                Ok(json!(matches.iter().map(|m| json!({"id":m.id,"revision":m.revision,"status":m.status,"title":m.content.title,"fact":m.content.fact,"conclusion":text_page(&m.content.conclusion,0,2048)})).collect::<Vec<_>>()))
            }
            "memory_conflicts" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ConflictPage {
                    #[serde(default)]
                    after: String,
                }
                let args: ConflictPage = serde_json::from_value(arguments)?;
                self.memory_checked.store(true, Ordering::Relaxed);
                Ok(serde_json::to_value(
                    self.store.conflicts(&self.project, &args.after, 8).await?,
                )?)
            }
            "memory_show" => {
                let args: Show = serde_json::from_value(arguments)?;
                Ok(serde_json::to_value(
                    self.store.show(&self.project, &args.id, None).await?,
                )?)
            }
            "source_neighbors" => {
                let args: SourceAnchor = serde_json::from_value(arguments)?;
                let sources = self
                    .store
                    .source_neighbors(&self.project, &args.receipt_id)
                    .await?;
                Ok(
                    json!({"sources":sources,"next_receipt_id":if sources.len()==8 {sources.last().map(|s|&s.receipt_id)} else {None}}),
                )
            }
            "source_read" => {
                let args: ReadSource = serde_json::from_value(arguments)?;
                let source = self.store.source(&self.project, &args.receipt_id).await?;
                let Some(message_id) = args.message_id else {
                    ensure!(
                        args.offset <= source.messages.len(),
                        "invalid source message offset"
                    );
                    return Ok(
                        json!({"receipt_id":source.receipt_id,"messages":source.messages.iter().skip(args.offset).take(32).map(|m|json!({"id":m.id,"role":m.role,"bytes":m.text.len()})).collect::<Vec<_>>(),"next_offset":if source.messages.len()-args.offset>32 {Some(args.offset+32)} else {None}}),
                    );
                };
                let message = source
                    .messages
                    .iter()
                    .find(|m| m.id == message_id)
                    .context("source message absent")?;
                ensure!(
                    message.text.is_char_boundary(args.offset),
                    "invalid UTF-8 offset"
                );
                Ok(
                    json!({"receipt_id":source.receipt_id,"message_id":message.id,"role":message.role,"page":text_page(&message.text,args.offset,8192)}),
                )
            }
            "repo_read" => {
                let args: ReadFile = serde_json::from_value(arguments)?;
                let root = self.root()?;
                let value = tokio::task::spawn_blocking(move || read_file(&root, &args))
                    .await?
                    .map_err(|error| {
                        if error.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) {
                            ToolInputError("repository path absent; locate an existing file with repo_search or use the supplied memory evidence".into()).into()
                        } else {
                            error
                        }
                    })?;
                self.record_inspection(name, &value);
                self.checked.store(true, Ordering::Relaxed);
                Ok(value)
            }
            "repo_search" => {
                let args: Query = serde_json::from_value(arguments).map_err(|_| {
                    ToolInputError("repo_search requires an object with one string query".into())
                })?;
                if args.query.is_empty() || args.query.len() > 512 {
                    return Err(ToolInputError(
                        "invalid repository query: use a nonempty literal of at most 512 UTF-8 bytes; empty queries cannot list files".into(),
                    ).into());
                }
                let root = self.root()?;
                let value =
                    tokio::task::spawn_blocking(move || search_files(&root, &args.query)).await??;
                self.record_inspection(name, &value);
                self.checked.store(true, Ordering::Relaxed);
                Ok(value)
            }
            _ => bail!("unknown project tool"),
        }
    }
}

fn allowed(name: &str) -> bool {
    !matches!(
        name,
        ".git" | ".env" | ".ssh" | "node_modules" | "target" | "vendor"
    ) && !name.starts_with(".env.")
        && !Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pem") || ext.eq_ignore_ascii_case("key"))
}

fn scoped_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(!relative.is_empty(), "missing repository path");
    let mut path = root.to_owned();
    for part in Path::new(relative).components() {
        let Component::Normal(name) = part else {
            bail!("repository path must be relative without traversal");
        };
        ensure!(allowed(&name.to_string_lossy()), "repository path excluded");
        path.push(name);
        ensure!(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "repository symlink excluded"
        );
    }
    ensure!(
        path.canonicalize()?.starts_with(root),
        "repository path escapes root"
    );
    Ok(path)
}

fn read_file(root: &Path, args: &ReadFile) -> Result<Value> {
    let path = scoped_path(root, &args.path)?;
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && args.offset <= metadata.len(),
        "invalid repository file or offset"
    );
    file.seek(SeekFrom::Start(args.offset))?;
    let mut bytes = Vec::new();
    file.take(8196).read_to_end(&mut bytes)?;
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()])?
        }
        Err(error) => return Err(error.into()),
    };
    ensure!(!text.contains('\0'), "binary repository file excluded");
    let page = text_page(text, 0, 8192);
    let read = page["text"].as_str().context("invalid file page")?;
    let next = args.offset + u64::try_from(read.len())?;
    Ok(
        json!({"path":args.path,"text":read,"next_offset":if next < metadata.len() {Some(next)} else {None},"size":metadata.len()}),
    )
}

fn text_page(text: &str, start: usize, limit: usize) -> Value {
    let mut end = (start.saturating_add(limit)).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({"text":&text[start..end],"next_offset":if end < text.len() {Some(end)} else {None}})
}

fn search_files(root: &Path, query: &str) -> Result<Value> {
    let mut digest = Sha256::new();
    let mut pending = vec![root.to_owned()];
    let mut matches = Vec::new();
    let mut visited = 0;
    let mut bytes_read = 0;
    let mut complete = true;
    'scan: while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            visited += 1;
            if visited > 2000 || bytes_read >= 2 * 1024 * 1024 || matches.len() >= 20 {
                complete = false;
                break 'scan;
            }
            let entry = entry?;
            if !allowed(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let path = entry.path();
            let size = entry.metadata()?.len();
            if size > 256 * 1024 {
                complete = false;
                continue;
            }
            let mut bytes = Vec::new();
            File::open(&path)?
                .take(256 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            digest.update(path.strip_prefix(root)?.as_os_str().as_encoded_bytes());
            digest.update(&bytes);
            bytes_read += bytes.len();
            if bytes.len() > 256 * 1024 {
                complete = false;
                continue;
            }
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            if text.contains('\0') {
                continue;
            }
            for (index, line) in text.lines().enumerate() {
                if line.contains(query) {
                    matches.push(json!({"path":path.strip_prefix(root)?.to_string_lossy(),"line":index+1,"text":text_page(line,0,512)["text"]}));
                    if matches.len() >= 20 {
                        complete = false;
                        break 'scan;
                    }
                }
            }
        }
    }
    Ok(
        json!({"query":query,"scanned_digest":format!("{:x}",digest.finalize()),"matches":matches,"complete":complete,"visited":visited,"bytes_read":bytes_read,"excludes":"symlinks, secrets, generated dependencies, binary files"}),
    )
}
