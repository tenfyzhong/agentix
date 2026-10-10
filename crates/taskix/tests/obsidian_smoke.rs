//! Opt-in desktop integration: uses an isolated temporary directory in an open
//! vault, closes only its own views, and removes only its generated test files.
use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

static DESKTOP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn obsidian(vault: &str, expression: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut child =
            Command::new(std::env::var("OBSIDIAN_BIN").unwrap_or_else(|_| "obsidian".into()))
                .arg(format!("vault={vault}"))
                .arg("eval")
                .arg(format!(
                    "code=(async()=>JSON.stringify((await ({expression})) ?? null))()"
                ))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("Obsidian CLI must be available and the vault open");
        let attempt_deadline = Instant::now() + Duration::from_secs(15);
        let timed_out = loop {
            if child.try_wait().unwrap().is_some() {
                break false;
            }
            if Instant::now() >= attempt_deadline {
                child.kill().unwrap();
                break true;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        if let Some(value) = stdout.lines().find_map(|line| line.strip_prefix("=> ")) {
            return serde_json::from_str(value).expect(&stdout);
        }
        assert!(output.status.success() || timed_out, "{stdout}");
        // Desktop IPC can briefly omit a result while changing the active tab.
        assert!(
            Instant::now() < deadline,
            "Missing Obsidian result for {expression}: stdout={stdout:?}, stderr={:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn obsidian_action(vault: &str, code: &str) {
    // Switching the active leaf can discard eval's result. Verify effects with
    // subsequent read-only eval calls, not the mutation's return value.
    let mut child =
        Command::new(std::env::var("OBSIDIAN_BIN").unwrap_or_else(|_| "obsidian".into()))
            .arg(format!("vault={vault}"))
            .arg("eval")
            .arg(format!("code={code}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        if Instant::now() >= deadline {
            // View switches sometimes apply but lose the CLI acknowledgement.
            // Kill only our waiting client; subsequent assertions verify the UI.
            child.kill().unwrap();
            child.wait().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for(vault: &str, expression: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let value = obsidian(vault, expression);
        if predicate(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "Obsidian did not reach expected state for {expression}: {value}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct DesktopFixture {
    vault: String,
    original_leaf: Value,
    leaf: Value,
    relative: String,
    output: tempfile::TempDir,
    metadata: tempfile::TempDir,
}

impl DesktopFixture {
    fn cli(&self, args: &[&str]) -> Value {
        let output = Command::new(
            std::env::var_os("TASKIX_OBSIDIAN_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_taskix").into()),
        )
        .arg("--config")
        .arg(self.metadata.path().join("config.toml"))
        .arg("--json")
        .args(args)
        .current_dir(self.metadata.path())
        .output()
        .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            response["projection_pending"].is_null(),
            "{args:?}: {response}"
        );
        response["result"].clone()
    }

    fn open(&mut self, path: &str, view_type: &str) {
        // Use a fresh owned tab for each view and restore the original during cleanup.
        obsidian_action(
            &self.vault,
            &format!("app.workspace.getLeafById({})?.detach()", self.leaf),
        );
        self.leaf = obsidian(&self.vault, "app.workspace.getLeaf('tab').id");
        wait_for(
            &self.vault,
            &format!("!!app.vault.getAbstractFileByPath({})", json!(path)),
            |v| v == true,
        );
        obsidian_action(
            &self.vault,
            &format!(
                "(async () => {{ const leaf = app.workspace.getLeafById({}); await leaf.setViewState({}); await app.workspace.revealLeaf(leaf); app.workspace.setActiveLeaf(leaf, {{focus:true}}); return true; }})()",
                self.leaf,
                json!({"type":view_type,"state":{"file":path,"mode":"preview"}})
            ),
        );
        if view_type == "markdown" {
            // Reading view lazily mounts embedded Bases below long metadata/graphs.
            let preview = format!(
                "app.workspace.getLeafById({}).view.contentEl.querySelector('.markdown-preview-view')",
                self.leaf
            );
            wait_for(&self.vault, &format!("!!({preview})?.scrollHeight"), |v| {
                v == true
            });
            obsidian(
                &self.vault,
                &format!(
                    "(()=>{{const el={preview};el.scrollTop=el.scrollHeight;return true;}})()"
                ),
            );
        }
    }
}

impl Drop for DesktopFixture {
    fn drop(&mut self) {
        // Do not panic a second time if the app was closed during a failed test.
        let expression = format!(
            "(async () => {{ await app.plugins.unloadPlugin(\"taskix-sync-smoke\"); delete app.plugins.manifests[\"taskix-sync-smoke\"]; delete window.taskixSyncSmoke; app.workspace.getLeafById({})?.detach(); for (const leaf of ['markdown','bases'].flatMap(type=>app.workspace.getLeavesOfType(type))) {{ if (leaf.view.file?.path.startsWith({})) leaf.detach(); }} const original = app.workspace.getLeafById({}); if (original) app.workspace.setActiveLeaf(original, {{focus:true}}); return true; }})()",
            self.leaf,
            json!(format!("{}/", self.relative)),
            self.original_leaf
        );
        let _ = std::panic::catch_unwind(|| obsidian_action(&self.vault, &expression));
    }
}

// Bases virtualizes horizontally as well as vertically. Visit each column so
// acceptance checks cover off-screen groups without depending on mounted cards.
fn native_board_snapshot(f: &DesktopFixture, bottom: bool) -> Value {
    let root = format!("app.workspace.getLeafById({}).view.contentEl", f.leaf);
    let widths = obsidian(
        &f.vault,
        &format!(
            "[...{root}.querySelectorAll('.bases-view[data-view-type=kanban]')].map(el=>el.scrollWidth)"
        ),
    );
    let mut columns = Vec::new();
    let mut cards = Vec::new();
    let mut counts = Vec::new();
    for (index, width) in widths.as_array().unwrap().iter().enumerate() {
        obsidian(
            &f.vault,
            &format!(
                "(()=>{{{root}.querySelectorAll('.bases-view[data-view-type=kanban]')[{index}].scrollIntoView({{block:'center',inline:'nearest'}});return true;}})()"
            ),
        );
        let mut view_columns = Vec::new();
        for offset in (0..=width.as_u64().unwrap() + 300).step_by(300) {
            obsidian(
                &f.vault,
                &format!(
                    "(()=>{{{root}.querySelectorAll('.bases-view[data-view-type=kanban]')[{index}].scrollLeft={offset};return true;}})()"
                ),
            );
            std::thread::sleep(Duration::from_millis(100));
            if bottom {
                obsidian(
                    &f.vault,
                    &format!(
                        "(()=>{{for(const el of {root}.querySelectorAll('.bases-kanban-container')[{index}].querySelectorAll('.bases-kanban-column-content')) el.scrollTop=el.scrollHeight;return true;}})()"
                    ),
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            let rendered = obsidian(
                &f.vault,
                &format!(
                    "[...{root}.querySelectorAll('.bases-kanban-container')[{index}].querySelectorAll('.bases-kanban-column')].filter(el=>el.querySelector('.bases-kanban-column-header[draggable]')).map(el=>({{status:el.querySelector('.bases-group-value').textContent,count:Number(el.querySelector('.bases-kanban-column-count').textContent),cards:[...el.querySelectorAll('.bases-kanban-card[draggable] .mod-title .internal-link')].map(link=>({{path:link.dataset.href,status:el.querySelector('.bases-group-value').textContent}}))}}))"
                ),
            );
            for column in rendered.as_array().unwrap() {
                if !view_columns.contains(&column["status"]) {
                    view_columns.push(column["status"].clone());
                    counts.push(json!({"status":column["status"],"count":column["count"]}));
                }
                for card in column["cards"].as_array().unwrap() {
                    if !cards.contains(card) {
                        cards.push(card.clone());
                    }
                }
            }
        }
        columns.extend(view_columns);
    }
    json!({"columns":columns,"cards":cards,"counts":counts})
}

fn native_card_paths(board: &Value) -> Value {
    json!(
        board["cards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|card| card["path"].clone())
            .collect::<Vec<_>>()
    )
}

fn wait_native_board(f: &DesktopFixture, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let value = native_board_snapshot(f, false);
        if predicate(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "native board did not render: {value}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires TASKIX_OBSIDIAN_VAULT and enabled Obsidian 1.14+ Bases plugins in an open desktop vault"]
fn native_bases_render_obsidian_and_resolves_task_note_links() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault =
        std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the open test vault explicitly");
    assert_eq!(
        obsidian(
            &vault,
            "!!app.internalPlugins.getPluginById('bases')?.enabled"
        ),
        true,
        "Enable Bases in Obsidian 1.14 or newer before running this test"
    );
    exercise_plugin_views(&vault, true);
    exercise_plugin_views(&vault, false);
}

#[test]
#[ignore = "requires an open TASKIX_OBSIDIAN_VAULT with Obsidian 1.14+ Bases enabled"]
fn taskix_sync_and_dual_boards_in_desktop() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault = std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the test vault");
    exercise_plugin_views(&vault, false);
}

#[test]
#[ignore = "requires an open TASKIX_OBSIDIAN_VAULT with Obsidian 1.14+ Bases enabled"]
fn whitespace_job_cancellation_preserves_terminal_tasks_in_desktop() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault = std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the test vault");
    let (f, project) = desktop_fixture(&vault);
    let (job, _) = desktop_finished_job(&f, &project, "Whitespace cancellation");
    f.cli(&["job", "reject", &job, "--reason", "Add cancelled work"]);
    let cancelled = f.cli(&["task", "add", "--job", &job, "--title", "Cancelled work"]);
    f.cli(&["task", "cancel", cancelled["id"].as_str().unwrap()]);
    f.cli(&[
        "job",
        "reject",
        &job,
        "--reason",
        "Keep active for cancellation",
    ]);
    assert_eq!(f.cli(&["job", "show", &job])["status"], "ACTIVE");
    let before = f.cli(&["task", "list", "--job", &job]);
    let task_history = || {
        f.cli(&["event", "list", "--job", &job, "--limit", "1000"])["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["task_id"].is_string())
            .cloned()
            .collect::<Vec<_>>()
    };
    let history_before = task_history();
    assert!(!history_before.is_empty());
    let path = f.cli(&["obsidian", "show", &job])["path"].clone();
    load_sync_plugin(&f);
    wait_for(
        &vault,
        &format!(
            "app.metadataCache.getFileCache(app.vault.getAbstractFileByPath({path}))?.frontmatter?.status"
        ),
        |status| status == "ACTIVE",
    );
    obsidian(
        &vault,
        &format!(
            "app.fileManager.processFrontMatter(app.vault.getAbstractFileByPath({path}), fm=>{{fm.status={};}})",
            json!("CANCELLED\n")
        ),
    );
    let lookup = format!(
        "[...window.taskixSyncSmoke.engine.notes.values()].find(n=>n.id==={})?.status",
        json!(job)
    );
    wait_for(&vault, &lookup, |status| status == "CANCELLED");
    assert_eq!(f.cli(&["job", "show", &job])["status"], "CANCELLED");
    assert_eq!(f.cli(&["task", "list", "--job", &job]), before);
    assert_eq!(task_history(), history_before);
    wait_for(
        &vault,
        &format!(
            "app.metadataCache.getFileCache(app.vault.getAbstractFileByPath({path}))?.frontmatter?.status"
        ),
        |status| status == "CANCELLED",
    );
    assert_eq!(obsidian(&vault, "window.taskixSmokeNotices"), json!([]));
}

#[test]
#[ignore = "requires an open TASKIX_OBSIDIAN_VAULT with Obsidian 1.14+ Bases enabled"]
fn inbox_checkbox_sync_in_desktop() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault = std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the test vault");
    let (f, project) = desktop_fixture(&vault);
    load_sync_plugin(&f);
    exercise_inbox_bridge(&f, project["id"].as_str().unwrap());
}

#[test]
#[ignore = "requires an open TASKIX_OBSIDIAN_VAULT with Obsidian 1.14+ Bases enabled"]
fn id_queries_and_status_sync_in_desktop() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault = std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the test vault");
    let (f, project) = desktop_fixture(&vault);
    let job = f.cli(&[
        "job",
        "create",
        "--project",
        project["id"].as_str().unwrap(),
        "--title",
        "ID query acceptance",
    ]);
    let task = f.cli(&[
        "task",
        "add",
        "--job",
        job["id"].as_str().unwrap(),
        "--title",
        "Leased task",
    ]);
    f.cli(&[
        "task",
        "add",
        "--job",
        job["id"].as_str().unwrap(),
        "--title",
        "Unleased task",
    ]);
    f.cli(&[
        "task",
        "claim",
        task["id"].as_str().unwrap(),
        "--executor",
        "agent:smoke",
        "--session",
        "smoke",
    ]);
    let task_note = f.cli(&["obsidian", "show", task["id"].as_str().unwrap()]);
    let job_note = f.cli(&["obsidian", "show", job["id"].as_str().unwrap()]);
    for note in [&task_note, &job_note] {
        wait_for(
            &vault,
            &format!(
                "app.metadataCache.getFileCache(app.vault.getAbstractFileByPath({}))?.frontmatter?.id",
                note["path"]
            ),
            |id| id == &note["id"],
        );
    }
    exercise_status_bridge(
        &f,
        &task,
        &job,
        task_note["path"].as_str().unwrap(),
        job_note["path"].as_str().unwrap(),
    );
}

fn exercise_dashboard(f: &mut DesktopFixture, project: &Value) {
    let dashboard = format!("{}/Dashboard.base", f.relative);
    let view_type = "bases";
    let board = format!(
        "{}/Projects/{}/Board.md",
        f.relative,
        project["key"].as_str().unwrap()
    );
    f.open(&dashboard, view_type);
    let expression = format!(
        "(() => {{const root=app.workspace.getLeafById({}).view.contentEl; const el=root.querySelector('.markdown-preview-view') ?? root; return {{text:el.textContent,date:el.querySelector('input[type=datetime-local]')?.value,links:[...el.querySelectorAll('.internal-link')].map(a=>({{name:a.textContent,href:a.dataset.href}}))}};}})()",
        f.leaf
    );
    let rendered = wait_for(&f.vault, &expression, |v| {
        v["links"].as_array().is_some_and(|links| {
            links
                .iter()
                .any(|link| link["name"] == "Rendering acceptance")
        })
    });
    let year = obsidian(
        &f.vault,
        &format!("new Date({} * 1000).getFullYear()", project["created_at"]),
    )
    .to_string();
    assert!(
        rendered["text"].as_str().unwrap().contains(&year),
        "missing activity date: {rendered}"
    );
    for header in ["Name", "Status", "Updated", "ACTIVE"] {
        assert!(
            rendered["text"].as_str().unwrap().contains(header),
            "{rendered}"
        );
    }
    assert_eq!(
        rendered["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["href"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1,
        "Dashboard must exclude other projects in the vault: {rendered}"
    );
    let expression = format!(
        "(() => {{const root=app.workspace.getLeafById({}).view.contentEl; return (root.querySelector('.markdown-preview-view') ?? root).textContent;}})()",
        f.leaf
    );
    wait_for(&f.vault, &expression, |v| {
        v.as_str()
            .is_some_and(|text| text.contains("Rendering acceptance"))
    });
    f.cli(&["project", "archive", project["id"].as_str().unwrap()]);
    wait_for(&f.vault, &expression, |v| {
        v.as_str()
            .is_some_and(|text| !text.contains("Rendering acceptance"))
    });
    f.cli(&["project", "unarchive", project["id"].as_str().unwrap()]);
    wait_for(&f.vault, &expression, |v| {
        v.as_str()
            .is_some_and(|text| text.contains("Rendering acceptance"))
    });
    obsidian_action(
        &f.vault,
        &format!(
            "(() => {{const link=app.workspace.getLeafById({}).view.contentEl.querySelector('.internal-link'); return app.workspace.openLinkText(link.dataset.href, {}, false);}})()",
            f.leaf,
            json!(dashboard)
        ),
    );
    wait_for(
        &f.vault,
        "app.workspace.getMostRecentLeaf()?.view.file?.path",
        |v| v == &board,
    );
    f.leaf = obsidian(&f.vault, "app.workspace.getMostRecentLeaf()?.id");
}

fn desktop_fixture(vault: &str) -> (DesktopFixture, Value) {
    let info = obsidian(
        vault,
        "({root:app.vault.adapter.basePath,leaf:app.workspace.getMostRecentLeaf()?.id})",
    );
    let root = PathBuf::from(info["root"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    let parent =
        std::env::var("TASKIX_OBSIDIAN_PARENT").unwrap_or_else(|_| "00-Inbox/agent".into());
    let parent = root.join(parent).canonicalize().unwrap();
    assert!(
        parent.starts_with(&root),
        "test output must stay inside the vault"
    );
    let output = tempfile::Builder::new()
        .prefix("taskix-smoke-")
        .tempdir_in(parent)
        .unwrap();
    let relative = output
        .path()
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    obsidian_action(
        vault,
        "app.workspace.setActiveLeaf(app.workspace.getLeaf('tab'),{focus:true})",
    );
    let leaf = obsidian(vault, "app.workspace.getMostRecentLeaf()?.id");
    assert_ne!(leaf, info["leaf"], "use a dedicated test tab");
    let f = DesktopFixture {
        vault: vault.to_owned(),
        original_leaf: info["leaf"].clone(),
        leaf,
        relative,
        output,
        metadata: tempfile::tempdir().unwrap(),
    };
    f.cli(&[
        "init",
        "--root",
        root.to_str().unwrap(),
        "--directory",
        &f.relative,
        "--database",
        f.metadata.path().join("tasks.sqlite3").to_str().unwrap(),
    ]);
    let project = f.cli(&[
        "project",
        "register",
        "--name",
        "Rendering acceptance",
        "--root",
        f.metadata.path().to_str().unwrap(),
    ]);
    (f, project)
}

#[allow(clippy::too_many_lines)] // Keep the opt-in desktop scenario and cleanup visible together.
fn exercise_plugin_views(vault: &str, dashboard_only: bool) {
    let (mut f, project) = desktop_fixture(vault);
    if dashboard_only {
        exercise_dashboard(&mut f, &project);
        return;
    }
    let job = f.cli(&[
        "job",
        "create",
        "--project",
        project["id"].as_str().unwrap(),
        "--title",
        "Task board rendering acceptance",
    ]);
    let task = f.cli(&[
        "task",
        "add",
        "--job",
        job["id"].as_str().unwrap(),
        "--title",
        "Render | Unicode \u{2603} [link] & <tag> [[internal]] *bold*",
    ]);
    let claim = f.cli(&[
        "task",
        "claim",
        task["id"].as_str().unwrap(),
        "--executor",
        "agent:smoke",
        "--session",
        "smoke",
    ]);
    let plan = f.cli(&[
        "plan",
        "create",
        task["id"].as_str().unwrap(),
        "--body",
        "---\ntitle: Rendering acceptance plan\ntags:\n  - para/inbox\n---\n\n# Rendering acceptance plan\n\nVerify link navigation.",
        "--session",
        "smoke",
        "--lease-token",
        claim["lease"]["token"].as_str().unwrap(),
    ]);
    f.cli(&[
        "task",
        "add",
        "--job",
        job["id"].as_str().unwrap(),
        "--title",
        "Task without a Plan",
    ]);
    let board = format!(
        "{}/Projects/{}/Board.md",
        f.relative,
        project["key"].as_str().unwrap()
    );
    f.open(&board, "markdown");
    let rendered = wait_native_board(&f, |v| {
        v["cards"].as_array().is_some_and(|a| a.len() == 3)
            && v["columns"].as_array().is_some_and(|a| a.len() == 11)
    });
    assert_eq!(
        rendered["columns"],
        json!([
            "ACTIVE",
            "PENDING_REVIEW",
            "COMPLETED",
            "CANCELLED",
            "TODO",
            "IN_PROGRESS",
            "BLOCKED",
            "WAITING_USER",
            "DONE",
            "FAILED",
            "CANCELLED"
        ])
    );
    let path = format!("{}/{}", f.relative, plan["path"].as_str().unwrap());
    assert!(
        rendered["cards"]
            .as_array()
            .unwrap()
            .iter()
            .any(|card| card["path"] == path && card["status"] == "IN_PROGRESS")
    );
    let job_path = format!("{}/{}", f.relative, job["document_path"].as_str().unwrap());
    f.open(&job_path, "markdown");
    let links = format!(
        "(() => {{const el=app.workspace.getLeafById({}).view.contentEl;return [...new Set([...el.querySelectorAll('.internal-link')].map(e=>app.metadataCache.getFirstLinkpathDest(decodeURIComponent(e.getAttribute('data-href')??e.getAttribute('href')??''),{})?.path).filter(p=>p?.includes('/Tasks/')))];}})()",
        f.leaf,
        json!(job_path)
    );
    let resolved = wait_for(&f.vault, &links, |v| {
        v.as_array().is_some_and(|a| a.len() == 2)
    });
    assert!(resolved.as_array().unwrap().contains(&json!(path)));
    exercise_status_bridge(&f, &task, &job, &path, &job_path);
    let task_info = obsidian(
        &f.vault,
        &format!(
            "(() => {{const path={};const file=app.vault.getAbstractFileByPath(path);const fm=app.metadataCache.getFileCache(file)?.frontmatter;return {{path,status:fm?.status,id:fm?.id}};}})()",
            json!(path)
        ),
    );
    assert_eq!(task_info["id"], task["id"]);
    assert_eq!(task_info["path"], path);
    assert_eq!(task_info["status"], "IN_PROGRESS");
    f.open(&path, "markdown");
    let body = format!(
        "app.workspace.getLeafById({}).view.contentEl.textContent.includes('Verify link navigation.')",
        f.leaf
    );
    wait_for(&f.vault, &body, |v| v == true);
    // Keep the TempDir alive through all app reads; Drop restores views first.
    assert!(f.output.path().exists());
}

#[test]
#[ignore = "requires a visible TASKIX_OBSIDIAN_VAULT with Mermaid diagram rendering enabled"]
fn mermaid_state_machine_diagrams_render_in_desktop() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault = std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the test vault");
    let (mut f, _) = desktop_fixture(&vault);
    std::fs::write(
        f.output.path().join("State machines.md"),
        include_str!("../../../docs/task-state-machines.md"),
    )
    .unwrap();
    let path = format!("{}/State machines.md", f.relative);
    f.open(&path, "markdown");
    obsidian(
        &f.vault,
        &format!(
            "(()=>{{app.workspace.getLeafById({}).view.contentEl.querySelector('.markdown-preview-view').scrollTop=0;return true;}})()",
            f.leaf
        ),
    );
    wait_for(
        &f.vault,
        &format!(
            "(()=>{{const el=app.workspace.getLeafById({}).view.contentEl; return {{count:el.querySelectorAll('.markdown-preview-view .mermaid svg, .markdown-preview-view .block-language-mermaid svg').length,html:el.querySelector('.markdown-preview-view')?.innerHTML.slice(-6000)}};}})()",
            f.leaf
        ),
        |v| v["count"] == 3,
    );
}

#[allow(clippy::too_many_lines)] // Keep the desktop mutation, rollback and database assertions together.
fn load_sync_plugin(f: &DesktopFixture) {
    let plugin_dir = f.output.path().join("test-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("main.js"),
        include_str!("../../../plugins/taskix-manager/obsidian/taskix-sync/main.js"),
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("styles.css"),
        include_str!("../../../plugins/taskix-manager/obsidian/taskix-sync/styles.css"),
    )
    .unwrap();
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../../plugins/taskix-manager/obsidian/taskix-sync/manifest.json"
    ))
    .unwrap();
    manifest["id"] = json!("taskix-sync-smoke");
    manifest["dir"] = json!(format!("{}/test-plugin", f.relative));
    let settings = json!({"cliPath":env!("CARGO_BIN_EXE_taskix"),"configPath":f.metadata.path().join("config.toml")});
    std::fs::write(plugin_dir.join("data.json"), settings.to_string()).unwrap();
    obsidian(
        &f.vault,
        &format!(
            "(async () => {{ app.plugins.manifests['taskix-sync-smoke']={manifest}; await app.plugins.loadPlugin('taskix-sync-smoke'); window.taskixSyncSmoke=app.plugins.plugins['taskix-sync-smoke']; return !!window.taskixSyncSmoke; }})()"
        ),
    );
    wait_for(
        &f.vault,
        "({ready:!!window.taskixSyncSmoke?.engine?.ready,loaded:!!window.taskixSyncSmoke,notices:[...activeDocument.querySelectorAll('.notice')].map(el=>el.textContent)})",
        |v| v["ready"] == true,
    );
    obsidian_action(&f.vault, "window.taskixSyncSmoke.checkConnection()");
    wait_for(
        &f.vault,
        "[...activeDocument.querySelectorAll('.notice')].some(el=>el.textContent.includes('Connected to taskix.'))",
        |v| v == true,
    );
    obsidian(
        &f.vault,
        "(() => { const io=window.taskixSyncSmoke.engine.io; const notice=io.notice; window.taskixSmokeNotices=[]; io.notice=m=>{window.taskixSmokeNotices.push(m); notice(m);}; return true;})()",
    );
}

fn exercise_status_bridge(
    f: &DesktopFixture,
    task: &Value,
    job: &Value,
    task_path: &str,
    job_path: &str,
) {
    load_sync_plugin(f);
    assert_eq!(
        obsidian(&f.vault, "typeof window.taskixSyncSmoke.engine.io.snapshot"),
        "undefined"
    );
    obsidian_action(
        &f.vault,
        &format!(
            "(async()=>{{for (const path of [{},{}]) await window.taskixSyncSmoke.inspectFile(app.vault.getAbstractFileByPath(path)); await window.taskixSyncSmoke.engine.flush(); return true;}})()",
            json!(task_path),
            json!(job_path)
        ),
    );
    wait_for(
        &f.vault,
        "!window.taskixSyncSmoke.engine.running && !window.taskixSyncSmoke.engine.pending.size",
        |v| v == true,
    );
    // The owning agent lease must reject a UI cancellation and restore dates/body.
    obsidian(
        &f.vault,
        &format!(
            "app.fileManager.processFrontMatter(app.vault.getAbstractFileByPath({}), fm=>{{fm.status='CANCELLED';fm.completedDate='2099-01-01';fm.smokeCustom='kept';}})",
            json!(task_path)
        ),
    );
    wait_for(&f.vault, "window.taskixSmokeNotices.length", |v| {
        v.as_u64().unwrap_or(0) >= 1
    });
    let props = wait_for(
        &f.vault,
        &format!(
            "app.metadataCache.getFileCache(app.vault.getAbstractFileByPath({}))?.frontmatter",
            json!(task_path)
        ),
        |v| v["status"] == "IN_PROGRESS" && v["completedDate"].is_null(),
    );
    assert_eq!(props["smokeCustom"], "kept");
    assert_eq!(
        f.cli(&["task", "show", task["id"].as_str().unwrap()])["status"],
        "IN_PROGRESS"
    );
    // Job cancellation is also rejected while its Task has an active lease.
    obsidian(
        &f.vault,
        &format!(
            "app.fileManager.processFrontMatter(app.vault.getAbstractFileByPath({}), fm=>{{fm.status='CANCELLED';}})",
            json!(job_path)
        ),
    );
    wait_for(&f.vault, "window.taskixSmokeNotices.length", |v| {
        v.as_u64().unwrap_or(0) >= 2
    });
    wait_for(
        &f.vault,
        &format!(
            "app.metadataCache.getFileCache(app.vault.getAbstractFileByPath({}))?.frontmatter?.status",
            json!(job_path)
        ),
        |v| v == "ACTIVE",
    );
    assert_eq!(
        f.cli(&["job", "show", job["id"].as_str().unwrap()])["status"],
        "ACTIVE"
    );
    // Unleased Task edits must go through the real executable and persist.
    let tasks = f.cli(&["task", "list", "--job", job["id"].as_str().unwrap()]);
    let unleased = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] != task["id"])
        .unwrap();
    let path = f.cli(&["obsidian", "show", unleased["id"].as_str().unwrap()])["path"].clone();
    obsidian(
        &f.vault,
        &format!(
            "app.fileManager.processFrontMatter(app.vault.getAbstractFileByPath({path}), fm=>{{fm.status='BLOCKED';}})"
        ),
    );
    wait_for(
        &f.vault,
        &format!(
            "app.metadataCache.getFileCache(app.vault.getAbstractFileByPath({path}))?.frontmatter?.revision > {}",
            unleased["revision"]
        ),
        |v| v == true,
    );
    assert_eq!(
        f.cli(&["task", "show", unleased["id"].as_str().unwrap()])["status"],
        "BLOCKED"
    );
    obsidian(
        &f.vault,
        "(async () => {await app.plugins.unloadPlugin(\"taskix-sync-smoke\"); delete app.plugins.manifests[\"taskix-sync-smoke\"]; delete window.taskixSyncSmoke; return true;})()",
    );
}

#[allow(clippy::too_many_lines)] // Keep the opt-in five-state workflow and assertions together.
fn exercise_inbox_bridge(f: &DesktopFixture, project: &str) {
    let entry = f.cli(&[
        "inbox",
        "add",
        "--project",
        project,
        "--content",
        "Manual Inbox smoke\nKeep details.",
    ]);
    let id = &entry["id"];
    let lookup = format!("[...window.taskixSyncSmoke.engine.notes.values()].find(n=>n.id==={id})");
    let note = wait_for(&f.vault, &lookup, |v| v["kind"] == "inbox");
    // The ID query can see a commit before the CLI finishes writing the file.
    // Wait for the displayed checkbox before simulating the next user click.
    wait_for(
        &f.vault,
        "!window.taskixSyncSmoke.engine.running && !window.taskixSyncSmoke.engine.pending.size",
        |v| v == true,
    );
    let path = &note["filePath"];
    let edit = |from: &str, to: &str| {
        wait_for(
            &f.vault,
            &format!(
                "(async()=> !window.taskixSyncSmoke.engine.running && !window.taskixSyncSmoke.engine.pending.size && (await app.vault.read(app.vault.getAbstractFileByPath({path}))).includes({}))()",
                json!(format!("- [{from}] Manual Inbox smoke"))
            ),
            |v| v == true,
        );
        obsidian(
            &f.vault,
            &format!(
                "app.vault.process(app.vault.getAbstractFileByPath({path}), source=>source.replace({},{}))",
                json!(format!("- [{from}] Manual Inbox smoke")),
                json!(format!("- [{to}] Manual Inbox smoke"))
            ),
        );
    };
    edit(" ", "x");
    wait_for(
        &f.vault,
        &format!(
            "(async()=> ({{note:({lookup}),ready:window.taskixSyncSmoke.engine.ready,pending:[...window.taskixSyncSmoke.engine.pending.values()],notices:window.taskixSmokeNotices,source:await app.vault.read(app.vault.getAbstractFileByPath({path}))}}))()"
        ),
        |v| v["note"]["status"] == "COMPLETED",
    );
    assert_eq!(
        f.cli(&["inbox", "list", "--project", project])[0]["status"],
        "COMPLETED"
    );
    let notices = obsidian(&f.vault, "window.taskixSmokeNotices.length")
        .as_u64()
        .unwrap();
    edit("x", "-");
    wait_for(&f.vault, "window.taskixSmokeNotices.length", |v| {
        v.as_u64().unwrap_or(0) > notices
    });
    wait_for(
        &f.vault,
        &format!(
            "(async()=> (await app.vault.read(app.vault.getAbstractFileByPath({path}))).includes('- [x] Manual Inbox smoke'))()"
        ),
        |v| v == true,
    );
    edit("x", " ");
    wait_for(&f.vault, &format!("({lookup})?.status"), |v| v == "TODO");
    let rows = f.cli(&["inbox", "list", "--project", project]);
    assert_eq!(rows[0]["status"], "TODO");
    assert_eq!(rows[0]["content"], "Manual Inbox smoke\nKeep details.");
    edit(" ", "/");
    wait_for(&f.vault, &format!("({lookup})?.status"), |v| v == "ACTIVE");
    let rows = f.cli(&["inbox", "list", "--project", project]);
    assert!(
        rows[0]["job_id"].is_null(),
        "manual ACTIVE does not create a Job"
    );
    f.cli(&[
        "inbox",
        "claim-next",
        "--project",
        project,
        "--executor",
        "agent:test",
        "--session",
        "desktop-inbox",
    ]);
    let rows = f.cli(&["inbox", "list", "--project", project]);
    let job = rows[0]["job_id"].as_str().unwrap();
    let task = f.cli(&[
        "task",
        "add",
        "--job",
        job,
        "--title",
        "Review checkbox smoke",
    ]);
    let task_id = task["id"].as_str().unwrap();
    let owned = f.cli(&[
        "task",
        "claim",
        task_id,
        "--session",
        "desktop-inbox",
        "--executor",
        "agent:test",
    ]);
    let token = owned["lease"]["token"].as_str().unwrap();
    f.cli(&[
        "plan",
        "create",
        task_id,
        "--body",
        "Verify five Inbox states",
        "--session",
        "desktop-inbox",
        "--lease-token",
        token,
    ]);
    for command in ["start", "done"] {
        f.cli(&[
            "task",
            command,
            task_id,
            "--session",
            "desktop-inbox",
            "--lease-token",
            token,
        ]);
    }
    wait_for(&f.vault, &format!("({lookup})?.status"), |v| {
        v == "PENDING_REVIEW"
    });
    wait_for(
        &f.vault,
        "!window.taskixSyncSmoke.engine.running && !window.taskixSyncSmoke.engine.pending.size",
        |v| v == true,
    );
    for (from, to, status) in [
        ("r", "/", "ACTIVE"),
        ("/", "r", "PENDING_REVIEW"),
        ("r", "x", "COMPLETED"),
    ] {
        edit(from, to);
        wait_for(&f.vault, &format!("({lookup})?.status"), |v| v == status);
        assert_eq!(f.cli(&["job", "show", job])["status"], status);
    }
}

#[test]
#[ignore = "requires a visible TASKIX_OBSIDIAN_VAULT with Obsidian 1.14+ Bases enabled"]
fn pending_review_dashboard_and_boards_sort_by_lifecycle_times() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault = std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the test vault");
    let (mut f, project) = desktop_fixture(&vault);
    let (first_job, first_task) = desktop_finished_job(&f, &project, "First");
    std::thread::sleep(Duration::from_secs(1));
    let (second_job, second_task) = desktop_finished_job(&f, &project, "Second");
    // The resubmitted Job has the newest update; Task completion times stay fixed.
    std::thread::sleep(Duration::from_secs(1));
    f.cli(&["task", "update", &first_task, "--name", "Updated first"]);
    f.cli(&["job", "reject", &first_job, "--reason", "Recheck"]);
    f.cli(&["job", "submit", &first_job]);
    let snapshot = f.cli(&["obsidian", "snapshot"]);
    let path = |id: &str| {
        snapshot["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|note| note["id"] == id)
            .unwrap()["path"]
            .clone()
    };
    let board = format!(
        "{}/Projects/{}/Board.md",
        f.relative,
        project["key"].as_str().unwrap()
    );
    f.open(&board, "markdown");
    let expression = |leaf: &Value| {
        format!(
            "[...app.workspace.getLeafById({leaf}).view.contentEl.querySelectorAll('.bases-kanban-card[draggable] .mod-title .internal-link')].map(el=>el.dataset.href)"
        )
    };
    let expected_board = json!([
        path(&first_job),
        path(&second_job),
        path(&second_task),
        path(&first_task)
    ]);
    let board = wait_native_board(&f, |v| native_card_paths(v) == expected_board);
    assert_eq!(native_card_paths(&board), expected_board);
    let dashboard = f.output.path().join("Dashboard.base");
    let mut base: Value =
        serde_yaml::from_str(&std::fs::read_to_string(&dashboard).unwrap()).unwrap();
    // Make the existing pending view the initial view in this isolated fixture.
    base["views"].as_array_mut().unwrap().rotate_left(1);
    std::fs::write(&dashboard, serde_yaml::to_string(&base).unwrap()).unwrap();
    f.open(&format!("{}/Dashboard.base", f.relative), "bases");
    let cards = wait_for(&vault, &expression(&f.leaf), |cards| {
        cards.as_array().is_some_and(|cards| cards.len() == 2)
    });
    assert_eq!(cards, json!([path(&first_job), path(&second_job)]));

    // Recent Jobs includes all projects and retains Jobs after review transitions.
    let other_root = f.metadata.path().join("other-project");
    std::fs::create_dir_all(&other_root).unwrap();
    let other = f.cli(&[
        "project",
        "register",
        "--root",
        other_root.to_str().unwrap(),
        "--name",
        "Other",
    ]);
    let (other_job, _) = desktop_finished_job(&f, &other, "Other project");
    let other_path = f.cli(&["job", "show", &other_job])["document_path"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_path = json!(format!("{}/{other_path}", f.relative));
    f.open(&format!("{}/Recent Jobs.base", f.relative), "bases");
    let cards = wait_for(&vault, &expression(&f.leaf), |cards| {
        cards.as_array().is_some_and(|cards| cards.len() == 3)
    });
    assert_eq!(
        cards,
        json!([other_path, path(&first_job), path(&second_job)])
    );
    f.cli(&["job", "approve", &second_job]);
    f.cli(&["job", "reject", &other_job, "--reason", "Needs repair"]);
    let cards = wait_for(&vault, &expression(&f.leaf), |cards| {
        cards.as_array().is_some_and(|cards| {
            cards == &vec![other_path.clone(), path(&first_job), path(&second_job)]
        })
    });
    assert_eq!(
        cards,
        json!([other_path, path(&first_job), path(&second_job)])
    );
}

fn desktop_finished_job(f: &DesktopFixture, project: &Value, name: &str) -> (String, String) {
    let job = f.cli(&[
        "job",
        "create",
        "--project",
        project["id"].as_str().unwrap(),
        "--title",
        name,
    ]);
    let id = job["id"].as_str().unwrap();
    let task = f.cli(&["task", "add", "--job", id, "--title", name]);
    let task_id = task["id"].as_str().unwrap();
    let claim = f.cli(&[
        "task",
        "claim",
        task_id,
        "--executor",
        "agent:smoke",
        "--session",
        "review-smoke",
    ]);
    let token = claim["lease"]["token"].as_str().unwrap();
    f.cli(&[
        "plan",
        "create",
        task_id,
        "--body",
        "Verify rendering.",
        "--session",
        "review-smoke",
        "--lease-token",
        token,
    ]);
    for command in ["start", "done"] {
        f.cli(&[
            "task",
            command,
            task_id,
            "--session",
            "review-smoke",
            "--lease-token",
            token,
        ]);
    }
    (id.into(), task_id.into())
}

#[test]
#[ignore = "requires a visible TASKIX_OBSIDIAN_VAULT with Obsidian 1.14+ Bases enabled"]
fn recent_jobs_cards_show_project_and_local_review_time() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault = std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the test vault");
    let (mut f, project) = desktop_fixture(&vault);
    let (job, _) = desktop_finished_job(&f, &project, "Card fields");
    load_sync_plugin(&f);
    f.open(&format!("{}/Recent Jobs.base", f.relative), "bases");
    let expression = format!(
        "[...app.workspace.getLeafById({}).view.contentEl.querySelectorAll('.bases-kanban-card[draggable]')].map(el=>el.innerText)",
        f.leaf
    );
    let cards = wait_for(&vault, &expression, |v| {
        v.as_array().is_some_and(|cards| cards.len() == 1)
    });
    let job = f.cli(&["job", "show", &job]);
    let expected = obsidian(
        &vault,
        &format!(
            "new Date({} * 1000).toLocaleString(\"sv-SE\")",
            job["pending_review_at"]
        ),
    );
    let text = cards[0].as_str().unwrap();
    assert_eq!(
        wait_native_board(&f, |v| v["columns"]
            .as_array()
            .is_some_and(|a| a.len() == 4))["columns"],
        json!(["ACTIVE", "PENDING_REVIEW", "COMPLETED", "CANCELLED"])
    );
    assert!(
        text.contains(expected.as_str().unwrap()),
        "missing review time {expected}: {text}"
    );
    assert!(
        text.contains(project["name"].as_str().unwrap()),
        "missing project: {text}"
    );
    let links = obsidian(
        &vault,
        &format!(
            "[...app.workspace.getLeafById({}).view.contentEl.querySelectorAll('.bases-kanban-card[draggable] .internal-link')].map(el=>el.dataset.href)",
            f.leaf
        ),
    );
    assert!(
        links
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link.as_str().is_some_and(|s| s.contains("/Board"))),
        "project link: {links}"
    );
}

#[test]
#[ignore = "requires an open TASKIX_OBSIDIAN_VAULT with Obsidian 1.14+ Bases enabled"]
fn recent_jobs_shows_all_cards_with_bounded_scrollable_height() {
    let _guard = DESKTOP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let vault = std::env::var("TASKIX_OBSIDIAN_VAULT").expect("choose the test vault");
    let (mut f, project) = desktop_fixture(&vault);
    let statuses = ["ACTIVE", "PENDING_REVIEW", "COMPLETED", "CANCELLED"];
    let mut expected = Vec::new();
    for status in statuses {
        for rank in 0..12 {
            let name = format!("{status}-{rank:02}");
            let id = if ["PENDING_REVIEW", "COMPLETED"].contains(&status) {
                desktop_finished_job(&f, &project, &name).0
            } else {
                f.cli(&[
                    "job",
                    "create",
                    "--project",
                    project["id"].as_str().unwrap(),
                    "--title",
                    &name,
                ])["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            };
            if status == "COMPLETED" {
                f.cli(&["job", "approve", &id]);
            }
            if status == "CANCELLED" {
                f.cli(&["job", "cancel", &id]);
            }
            let job = f.cli(&["job", "show", &id]);
            let relative = job["document_path"].as_str().unwrap();
            let path = f.output.path().join(relative);
            // Distinct fixture times exercise canonical timestamp sorting.
            let source = std::fs::read_to_string(&path).unwrap();
            let source = source
                .lines()
                .map(|line| {
                    if line.starts_with("updated_at:") {
                        format!("updated_at: '2026-09-07T12:00:{rank:02}+08:00'")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            std::fs::write(path, source).unwrap();
            expected.push(format!("{}/{relative}", f.relative));
        }
    }
    load_sync_plugin(&f);
    f.open(&format!("{}/Recent Jobs.base", f.relative), "bases");
    let layout = obsidian(
        &vault,
        &format!(
            "(() => {{const root=app.workspace.getLeafById({}).view.containerEl; const board=root.querySelector('.bases-view[data-view-type=kanban]'); return {{managed:root.classList.contains('taskix-board'),height:board.clientHeight,max:Math.min(600,innerHeight*.7),columns:[...board.querySelectorAll('.bases-kanban-column-content')].filter(el=>el.closest('.bases-kanban-column').querySelector('.bases-kanban-column-header[draggable]')).map(el=>({{height:el.clientHeight,scroll:el.scrollHeight,overflow:getComputedStyle(el).overflowY}}))}};}})()",
            f.leaf
        ),
    );
    assert_eq!(layout["managed"], true);
    assert!(
        layout["height"].as_f64().unwrap() <= layout["max"].as_f64().unwrap() + 1.0,
        "{layout}"
    );
    for column in layout["columns"].as_array().unwrap() {
        assert!(
            column["scroll"].as_u64().unwrap() > column["height"].as_u64().unwrap(),
            "{layout}"
        );
        assert_eq!(column["overflow"], "auto", "{layout}");
    }
    let rendered = wait_native_board(&f, |v| {
        v["columns"].as_array().is_some_and(|a| a.len() == 4)
    });
    assert_eq!(
        rendered["counts"],
        json!(statuses.map(|status| json!({"status":status,"count":12})))
    );
    // Visit both axes before verifying each status's oldest Job is reachable.
    let oldest: Vec<_> = expected.chunks(12).map(|chunk| chunk[0].clone()).collect();
    let root = format!("app.workspace.getLeafById({}).view.contentEl", f.leaf);
    obsidian(
        &vault,
        &format!(
            "(()=>{{for(const el of {root}.querySelectorAll('.bases-kanban-column-content')) el.scrollTop=el.scrollHeight;return true;}})()"
        ),
    );
    let rendered = native_board_snapshot(&f, true);
    for path in oldest {
        assert!(
            rendered["cards"]
                .as_array()
                .unwrap()
                .iter()
                .any(|card| card["path"] == path),
            "missing {path}: {rendered}"
        );
    }
}
