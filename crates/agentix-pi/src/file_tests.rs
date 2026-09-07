use super::*;
use std::collections::BTreeMap;
use std::fs::{self, FileTimes};

static HEADER_BARRIERS: std::sync::Mutex<BTreeMap<PathBuf, Arc<std::sync::Barrier>>> =
    std::sync::Mutex::new(BTreeMap::new());
static SPAWNS: std::sync::Mutex<BTreeMap<PathBuf, usize>> = std::sync::Mutex::new(BTreeMap::new());

pub(super) fn wait_header_scan(root: &Path) {
    let barrier = HEADER_BARRIERS.lock().unwrap().get(root).cloned();
    if let Some(barrier) = barrier {
        barrier.wait();
    }
}

pub(super) fn record_spawn(root: &Path) {
    *SPAWNS.lock().unwrap().entry(root.to_owned()).or_default() += 1;
}

#[cfg(unix)]
#[tokio::test]
async fn concurrent_attachment_starts_only_one_process() {
    let dir = fixture();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    HEADER_BARRIERS
        .lock()
        .unwrap()
        .insert(dir.path().to_owned(), barrier.clone());
    let adapter = Arc::new(PiRpcAdapter::new(PiFlavor::Pi, "/usr/bin/true", dir.path()));
    let first = adapter.clone();
    let first = tokio::spawn(async move { first.attach(&SessionId::new("session-0")).await });
    let second = adapter.clone();
    let second = tokio::spawn(async move { second.attach(&SessionId::new("session-0")).await });
    tokio::task::spawn_blocking(move || barrier.wait())
        .await
        .unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    HEADER_BARRIERS.lock().unwrap().remove(dir.path());
    let count = SPAWNS.lock().unwrap()[dir.path()];
    assert_eq!(
        count, 1,
        "concurrent attachment must not orphan a duplicate process"
    );
}

pub(super) static SUMMARY_READS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
pub(super) static SUMMARY_THREADS: std::sync::Mutex<BTreeMap<PathBuf, std::thread::ThreadId>> =
    std::sync::Mutex::new(BTreeMap::new());
static HISTORY_PEAK: std::sync::Mutex<BTreeMap<PathBuf, usize>> =
    std::sync::Mutex::new(BTreeMap::new());

pub(super) fn record_history_peak(path: &Path, count: usize) {
    let mut peaks = HISTORY_PEAK.lock().unwrap();
    let peak = peaks.entry(path.to_owned()).or_default();
    *peak = (*peak).max(count);
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for number in 0..40 {
        let path = dir.path().join(format!("{number}.jsonl"));
        let mut lines = vec![
            json!({"type":"session","id":format!("session-{number}"),"cwd":"/tmp"}).to_string(),
        ];
        for _ in 0..100 {
            lines.push(
                json!({"type":"message","message":{"role":"user","content":"Question"}})
                    .to_string(),
            );
        }
        fs::write(&path, lines.join("\n")).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(number + 1)),
            )
            .unwrap();
    }
    dir
}

fn summary_reads(root: &Path) -> usize {
    SUMMARY_READS
        .lock()
        .unwrap()
        .iter()
        .filter(|path| path.starts_with(root))
        .count()
}

#[tokio::test]
async fn list_page_reads_only_selected_session_bodies() {
    let dir = fixture();
    let adapter = PiRpcAdapter::new(PiFlavor::Pi, "unused", dir.path());
    let page = adapter.list_sessions(Some("5".into()), 3).await.unwrap();
    assert_eq!(
        page.sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        vec!["session-34", "session-33", "session-32"]
    );
    assert_eq!(page.next_cursor.as_deref(), Some("8"));
    assert_eq!(
        summary_reads(dir.path()),
        3,
        "only the selected page needs session bodies"
    );
    assert!(
        SUMMARY_THREADS
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.starts_with(dir.path()))
            .all(|(_, thread)| *thread != std::thread::current().id()),
        "file parsing must run outside the async executor thread"
    );
}

#[tokio::test]
async fn history_location_does_not_read_other_session_bodies() {
    let dir = fixture();
    let adapter = PiRpcAdapter::new(PiFlavor::Pi, "unused", dir.path());
    let history = adapter
        .read_history(&SessionId::new("session-0"), None, 3)
        .await
        .unwrap();
    assert_eq!(history.turns.len(), 3);
    assert_eq!(
        summary_reads(dir.path()),
        0,
        "history location needs headers, not summaries"
    );
}

#[test]
fn history_pages_do_not_retain_all_turn_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.jsonl");
    let lines = (0..1000).flat_map(|number| [
        json!({"type":"message","message":{"role":"user","content":format!("Question {number}")}}).to_string(),
        json!({"type":"message","message":{"role":"assistant","content":format!("Answer {number}")}}).to_string(),
    ]).collect::<Vec<_>>();
    fs::write(&path, lines.join("\n")).unwrap();
    for (cursor, last, older, newer) in [
        (None, 999, "995", None),
        (Some("995"), 994, "990", Some("1000")),
        (Some("5000"), 999, "995", None),
    ] {
        let page = read_history_file(&path, cursor, 5).unwrap();
        assert_eq!(page.turns.len(), 5);
        assert_eq!(page.turns[4].id, format!("turn-{}", last + 1));
        assert_eq!(
            page.turns[4].agent_text.as_deref(),
            Some(format!("Answer {last}").as_str())
        );
        assert_eq!(page.older_cursor.as_deref(), Some(older));
        assert_eq!(page.newer_cursor.as_deref(), newer);
    }
    let peak = HISTORY_PEAK.lock().unwrap()[&path];
    assert!(
        peak <= 5,
        "history parsing retained {peak} turns instead of one page"
    );
    for cursor in [Some("0"), Some("5"), None] {
        assert!(
            read_history_file(&path, cursor, 0)
                .unwrap()
                .turns
                .is_empty()
        );
    }
    fs::write(&path, "invalid JSON").unwrap();
    assert!(matches!(
        read_history_file(&path, Some("invalid cursor"), 5),
        Err(PiError::Json(_))
    ));
}

#[tokio::test]
async fn header_discovery_observes_file_changes_and_newest_duplicate_ids() {
    let dir = fixture();
    let adapter = PiRpcAdapter::new(PiFlavor::OhMyPi, "unused", dir.path());
    let path = dir.path().join("new.jsonl");
    fs::write(
        &path,
        [
            json!({"type":"title","title":"Fresh title"}).to_string(),
            json!({"type":"session","id":"session-0","cwd":"/new"}).to_string(),
            json!({"type":"message","message":{"role":"user","content":"New question"}})
                .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();
    let page = adapter.list_sessions(None, 1).await.unwrap();
    assert_eq!(page.sessions[0].name.as_deref(), Some("Fresh title"));
    assert_eq!(page.sessions[0].preview.as_deref(), Some("New question"));
    let history = adapter
        .read_history(&SessionId::new("session-0"), None, 1)
        .await
        .unwrap();
    assert_eq!(history.turns[0].user_text.as_deref(), Some("New question"));
    fs::write(&path, "{\"type\":\"session\",\"id\":\"replacement\"}\n").unwrap();
    assert_eq!(
        adapter.list_sessions(None, 1).await.unwrap().sessions[0]
            .id
            .as_str(),
        "replacement"
    );
    fs::remove_file(path).unwrap();
    assert_eq!(
        adapter.list_sessions(None, 1).await.unwrap().sessions[0]
            .id
            .as_str(),
        "session-39"
    );
    assert!(
        adapter
            .read_history(&SessionId::new("replacement"), None, 1)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn empty_or_invalid_list_pages_do_not_read_bodies() {
    let dir = fixture();
    let adapter = PiRpcAdapter::new(PiFlavor::Pi, "unused", dir.path());
    let zero = adapter.list_sessions(None, 0).await.unwrap();
    assert!(zero.sessions.is_empty());
    assert_eq!(zero.next_cursor.as_deref(), Some("0"));
    let beyond = adapter.list_sessions(Some("999".into()), 5).await.unwrap();
    assert!(beyond.sessions.is_empty());
    assert!(beyond.next_cursor.is_none());
    assert!(adapter.list_sessions(Some("bad".into()), 5).await.is_err());
    assert_eq!(summary_reads(dir.path()), 0);
}
