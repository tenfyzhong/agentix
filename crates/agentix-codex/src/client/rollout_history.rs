//! Bounded, incremental recovery of persisted display events omitted by summary RPCs.
use std::collections::{HashMap, VecDeque};
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::SystemTime;

use agentix_domain::{HistoryPage, ItemSummary, SessionId};
use serde_json::{Value, json};

use super::CodexClient;
use crate::protocol::item_summary;

const MAX_SESSIONS: usize = 16;
const MAX_SCAN: u64 = 8 * 1024 * 1024;
const MAX_LINE: usize = 256 * 1024;
const MAX_TEXT: usize = 16 * 1024;
const MAX_ITEMS: usize = 512;
const MAX_CACHE_BYTES: usize = 1024 * 1024;

type Snapshot = HashMap<String, Vec<ItemSummary>>;

#[derive(Default)]
pub(super) struct RolloutHistory {
    // Paths and scans share one bounded LRU; no background task or extra polling.
    sessions: VecDeque<(SessionId, Option<PathBuf>, Arc<Mutex<Scan>>)>,
}

impl RolloutHistory {
    fn session(
        &mut self,
        session: &SessionId,
    ) -> &mut (SessionId, Option<PathBuf>, Arc<Mutex<Scan>>) {
        if let Some(index) = self.sessions.iter().position(|entry| &entry.0 == session) {
            let entry = self.sessions.remove(index).unwrap();
            self.sessions.push_back(entry);
        } else {
            if self.sessions.len() == MAX_SESSIONS {
                self.sessions.pop_front();
            }
            self.sessions
                .push_back((session.clone(), None, Arc::default()));
        }
        self.sessions.back_mut().unwrap()
    }

    pub(super) fn remember_path(&mut self, session: &SessionId, path: Option<&str>) {
        let entry = self.session(session);
        let path = path.filter(|path| !path.is_empty()).map(PathBuf::from);
        if entry.1 != path {
            entry.1 = path;
            entry.2 = Arc::default();
        }
    }
}

impl CodexClient {
    pub(super) async fn restore_rollout_history(
        &self,
        session: &SessionId,
        page: &mut HistoryPage,
    ) {
        if page.turns.is_empty() {
            return;
        }
        let known = self
            .rollout_history
            .lock()
            .await
            .sessions
            .iter()
            .any(|entry| &entry.0 == session);
        if !known {
            // Attach and session metadata normally supply this already. History-only
            // reads need one metadata RPC, including for a newly discovered completion.
            if !matches!(
                tokio::time::timeout(Duration::from_millis(100), self.read_thread(session, false))
                    .await,
                Ok(Ok(_))
            ) {
                return;
            }
        }
        let (path, scan) = {
            let mut cache = self.rollout_history.lock().await;
            let (_, path, scan) = cache.session(session);
            (path.clone(), Arc::clone(scan))
        };
        let Some(path) = path else { return };
        let session = session.to_string();
        let wanted = page
            .turns
            .iter()
            .map(|turn| turn.id.clone())
            .collect::<Vec<_>>();
        // Disk I/O and JSON decoding never run on the event-consumer executor.
        let read = tokio::task::spawn_blocking(move || {
            let mut scan = scan.lock().unwrap();
            scan.read(&path, &session, &wanted)
        });
        // Recovery is optional. A slow local disk cannot hold up the available answer;
        // the bounded worker can still populate the cache for the next content read.
        let snapshot = tokio::time::timeout(Duration::from_millis(250), read)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(Result::ok)
            .unwrap_or_default();
        for turn in &mut page.turns {
            let Some(items) = snapshot.get(&turn.id) else {
                continue;
            };
            // Summary RPCs preserve inputs but omit intermediate assistant messages.
            // Native event identities preserve order and deduplicate the final answer.
            let recovered_ids = items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<std::collections::HashSet<_>>();
            let mut merged = turn
                .items
                .iter()
                .filter(|item| item.kind == "userMessage")
                .cloned()
                .collect::<Vec<_>>();
            merged.extend(items.iter().cloned());
            // Older app-server history reconstructs synthetic item IDs. Correlate
            // only the final answer, never collapse repeated intermediate answers.
            let final_answer = items.iter().rev().find(|item| item.kind == "agentMessage");
            merged.extend(
                turn.items
                    .iter()
                    .filter(|item| {
                        item.kind != "userMessage"
                            && !recovered_ids.contains(item.id.as_str())
                            && !(item.kind == "agentMessage"
                                && final_answer.is_some_and(|answer| answer.text == item.text))
                    })
                    .cloned(),
            );
            turn.items = merged;
        }
    }
}

#[derive(Default)]
struct Scan {
    identity: Option<(u64, u64)>,
    modified: Option<SystemTime>,
    length: u64,
    offset: u64,
    skip_fragment: bool,
    verified: bool,
    items: VecDeque<(String, ItemSummary)>,
    bytes: usize,
}

fn identity(metadata: &Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        (0, 0)
    }
}

impl Scan {
    fn read(&mut self, path: &Path, session: &str, wanted: &[String]) -> std::io::Result<Snapshot> {
        if !std::fs::metadata(path)?.is_file() {
            return Ok(HashMap::new());
        }
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        let length = metadata.len();
        let modified = metadata.modified().ok();
        if self.identity != Some(identity(&metadata))
            || length < self.length
            || (length == self.length && modified != self.modified)
        {
            *self = Self::default();
        }
        if !self.verified {
            let mut first = Vec::new();
            BufReader::new((&mut file).take(MAX_LINE as u64)).read_until(b'\n', &mut first)?;
            let meta = serde_json::from_slice::<Value>(&first).unwrap_or_default();
            if meta["type"] != "session_meta" || meta["payload"]["id"] != session {
                return Ok(HashMap::new());
            }
            self.verified = true;
        }
        self.identity = Some(identity(&metadata));
        self.modified = modified;
        self.length = length;
        let start = self.offset.max(length.saturating_sub(MAX_SCAN));
        let skip_first = self.skip_fragment || start != self.offset;
        file.seek(SeekFrom::Start(start))?;
        let mut data = Vec::new();
        file.take(length.saturating_sub(start))
            .read_to_end(&mut data)?;
        let mut consumed = 0;
        for (index, line) in data.split_inclusive(|byte| *byte == b'\n').enumerate() {
            if line.last() != Some(&b'\n') {
                // Retry a small unfinished record; skip a giant partial media line.
                self.skip_fragment = line.len() > MAX_LINE || (index == 0 && skip_first);
                if self.skip_fragment {
                    consumed += line.len();
                }
                break;
            }
            consumed += line.len();
            self.skip_fragment = false;
            if (index == 0 && skip_first) || line.len() > MAX_LINE {
                continue;
            }
            let Ok(entry) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let payload = &entry["payload"];
            if entry["type"] != "event_msg"
                || payload["type"] != "item_completed"
                || payload["thread_id"] != session
            {
                continue;
            }
            let Some(turn) = payload["turn_id"].as_str() else {
                continue;
            };
            let Some(item) = display_item(&payload["item"]) else {
                continue;
            };
            if let Some(existing) = self
                .items
                .iter_mut()
                .find(|(id, old)| id == turn && old.id == item.id)
            {
                self.bytes -= item_bytes(&existing.0, &existing.1);
                existing.1 = item;
                self.bytes += item_bytes(&existing.0, &existing.1);
            } else {
                self.bytes += item_bytes(turn, &item);
                self.items.push_back((turn.to_owned(), item));
            }
            while self.items.len() > MAX_ITEMS || self.bytes > MAX_CACHE_BYTES {
                let (turn, item) = self.items.pop_front().unwrap();
                self.bytes -= item_bytes(&turn, &item);
            }
        }
        self.offset = start + consumed as u64;
        let mut snapshot = HashMap::new();
        for (turn, item) in &self.items {
            if wanted.contains(turn) {
                snapshot
                    .entry(turn.clone())
                    .or_insert_with(Vec::new)
                    .push(item.clone());
            }
        }
        Ok(snapshot)
    }
}

fn item_bytes(turn: &str, item: &ItemSummary) -> usize {
    turn.len()
        + item.id.len()
        + item.kind.len()
        + item.text.as_ref().map_or(0, String::len)
        + item.status.as_ref().map_or(0, String::len)
}

fn display_item(native: &Value) -> Option<ItemSummary> {
    let mut item = native.clone();
    let kind = match native["type"].as_str()? {
        "Reasoning" => {
            item["summary"] = native["summary_text"].clone();
            "reasoning"
        }
        "AgentMessage" => {
            item["text"] = json!(
                native["content"]
                    .as_array()?
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            "agentMessage"
        }
        "CommandExecution" => {
            item["command"] = json!(
                native["command"]
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            item["aggregatedOutput"] = native["aggregated_output"].clone();
            "commandExecution"
        }
        "FileChange" => "fileChange",
        "McpToolCall" => "mcpToolCall",
        "DynamicToolCall" => "dynamicToolCall",
        "WebSearch" => "webSearch",
        "Plan" => "plan",
        // Ignore user/system inputs, encrypted reasoning, and media output.
        _ => return None,
    };
    item["type"] = json!(kind);
    let mut summary = item_summary(&item, "rollout history").ok()?;
    if let Some(status) = &mut summary.status {
        let mut chars = status.chars();
        if let Some(first) = chars.next() {
            *status = first.to_lowercase().collect::<String>() + chars.as_str();
        }
    }
    if kind != "agentMessage"
        && let Some(text) = &mut summary.text
    {
        let mut end = text.len().min(MAX_TEXT);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn meta(session: &str) -> String {
        format!(
            "{}\n",
            json!({"type":"session_meta","payload":{"id":session}})
        )
    }

    fn event(turn: &str, id: &str, text: &str) -> String {
        format!(
            "{}\n",
            json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":"s","turn_id":turn,"item":{"type":"Reasoning","id":id,"summary_text":[text]}}})
        )
    }

    #[test]
    fn incremental_scan_preserves_ids_order_updates_and_turn_isolation() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            "{}{}{}",
            meta("s"),
            event("t", "r", "first"),
            event("other", "r", "unrelated")
        )
        .unwrap();
        let mut scan = Scan::default();
        let wanted = vec!["t".into()];
        let initial = scan.read(file.path(), "s", &wanted).unwrap();
        assert_eq!(initial["t"][0].text.as_deref(), Some("first"));
        assert!(!initial.contains_key("other"));
        let offset = scan.offset;
        assert_eq!(scan.read(file.path(), "s", &wanted).unwrap(), initial);
        assert_eq!(scan.offset, offset);
        write!(
            file,
            "{}{}",
            event("t", "r", "updated"),
            event("t", "next", "second")
        )
        .unwrap();
        let updated = scan.read(file.path(), "s", &wanted).unwrap();
        assert_eq!(updated["t"].len(), 2);
        assert_eq!(updated["t"][0].id, "r");
        assert_eq!(updated["t"][0].text.as_deref(), Some("updated"));
        assert_eq!(updated["t"][1].id, "next");
        assert_eq!(scan.offset, file.as_file().metadata().unwrap().len());
    }

    #[test]
    fn incomplete_records_are_retried_after_append() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let entry = event("t", "r", "complete");
        write!(file, "{}{}", meta("s"), &entry[..entry.len() / 2]).unwrap();
        let mut scan = Scan::default();
        let wanted = vec!["t".into()];
        assert!(scan.read(file.path(), "s", &wanted).unwrap().is_empty());
        write!(file, "{}", &entry[entry.len() / 2..]).unwrap();
        assert_eq!(
            scan.read(file.path(), "s", &wanted).unwrap()["t"][0]
                .text
                .as_deref(),
            Some("complete")
        );
    }

    #[test]
    fn large_media_lines_are_skipped_and_recent_display_events_are_restored() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            "{}{}\n{}",
            meta("s"),
            "x".repeat(usize::try_from(MAX_SCAN).unwrap() + MAX_LINE),
            event("t", "r", "after media")
        )
        .unwrap();
        let mut scan = Scan::default();
        let snapshot = scan.read(file.path(), "s", &["t".into()]).unwrap();
        assert_eq!(snapshot["t"][0].text.as_deref(), Some("after media"));
        assert_eq!(scan.offset, file.as_file().metadata().unwrap().len());
        assert_eq!(scan.items.len(), 1);
    }

    #[test]
    fn partial_large_lines_do_not_hide_following_events() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{}{}", meta("s"), "x".repeat(MAX_LINE + 1)).unwrap();
        let mut scan = Scan::default();
        let wanted = vec!["t".into()];
        assert!(scan.read(file.path(), "s", &wanted).unwrap().is_empty());
        assert!(scan.skip_fragment);
        write!(file, "trailing media\n{}", event("t", "r", "next")).unwrap();
        assert_eq!(
            scan.read(file.path(), "s", &wanted).unwrap()["t"][0]
                .text
                .as_deref(),
            Some("next")
        );
    }

    #[test]
    fn foreign_sessions_and_threads_are_not_restored() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{}{}", meta("foreign"), event("t", "r", "private")).unwrap();
        let mut scan = Scan::default();
        assert!(
            scan.read(file.path(), "s", &["t".into()])
                .unwrap()
                .is_empty()
        );
        std::fs::write(
            file.path(),
            format!("{}{}", meta("foreign"), event("t", "r", "private")),
        )
        .unwrap();
        assert!(
            Scan::default()
                .read(file.path(), "foreign", &["t".into()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn truncation_and_replacement_invalidate_cached_content() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout");
        std::fs::write(
            &path,
            format!("{}{}", meta("s"), event("t", "old", "old content")),
        )
        .unwrap();
        let mut scan = Scan::default();
        let wanted = vec!["t".into()];
        assert_eq!(scan.read(&path, "s", &wanted).unwrap()["t"][0].id, "old");
        std::fs::write(&path, meta("s")).unwrap();
        assert!(scan.read(&path, "s", &wanted).unwrap().is_empty());
        let replacement = directory.path().join("replacement");
        std::fs::write(
            &replacement,
            format!("{}{}", meta("s"), event("t", "new", "new content")),
        )
        .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(scan.read(&path, "s", &wanted).unwrap()["t"][0].id, "new");
    }

    #[test]
    fn display_cache_has_byte_item_and_session_limits() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{}", meta("s")).unwrap();
        for i in 0..=MAX_ITEMS {
            write!(file, "{}", event("t", &i.to_string(), "text")).unwrap();
        }
        let mut scan = Scan::default();
        scan.read(file.path(), "s", &["t".into()]).unwrap();
        assert_eq!(scan.items.len(), MAX_ITEMS);
        assert_eq!(scan.items[0].1.id, "1");
        for i in 0..100 {
            write!(
                file,
                "{}",
                event("t", &format!("large-{i}"), &"雪".repeat(MAX_TEXT))
            )
            .unwrap();
        }
        scan.read(file.path(), "s", &["t".into()]).unwrap();
        assert!(scan.bytes <= MAX_CACHE_BYTES);
        assert!(
            scan.items
                .iter()
                .all(|(_, item)| item.text.as_ref().unwrap().len() <= MAX_TEXT)
        );
        let mut cache = RolloutHistory::default();
        for i in 0..=MAX_SESSIONS {
            cache.remember_path(&SessionId::new(i.to_string()), Some("/rollout"));
        }
        assert_eq!(cache.sessions.len(), MAX_SESSIONS);
        assert!(cache.sessions.iter().all(|entry| entry.0.as_str() != "0"));
    }

    #[test]
    fn native_display_fields_match_live_output_and_hide_raw_reasoning() {
        let reason = display_item(&json!({"type":"Reasoning","id":"r","summary_text":["visible"],"raw_content":["hidden"]})).unwrap();
        assert_eq!(reason.text.as_deref(), Some("visible"));
        let command = display_item(&json!({"type":"CommandExecution","id":"c","command":["cargo","test"],"status":"Completed","aggregated_output":"passed"})).unwrap();
        assert_eq!(command.text.as_deref(), Some("cargo test\npassed"));
        assert_eq!(command.status.as_deref(), Some("completed"));
        assert!(
            display_item(&json!({"type":"ImageGeneration","id":"media","result":"image"}))
                .is_none()
        );
    }
}
