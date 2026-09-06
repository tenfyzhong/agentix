//! Opt-in desktop integration: uses an isolated temporary directory in an open
//! vault, closes only its own views, and removes only its generated test files.
use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

fn obsidian(vault: &str, expression: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output =
            Command::new(std::env::var("OBSIDIAN_BIN").unwrap_or_else(|_| "obsidian".into()))
                .arg(format!("vault={vault}"))
                .arg("eval")
                .arg(format!(
                    "code=(async()=>JSON.stringify((await ({expression})) ?? null))()"
                ))
                .output()
                .expect("Obsidian CLI must be available and the vault open");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(output.status.success(), "{stdout}");
        if let Some(value) = stdout.lines().find_map(|line| line.strip_prefix("=> ")) {
            return serde_json::from_str(value).expect(&stdout);
        }
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
        let output = Command::new(env!("CARGO_BIN_EXE_taskcli"))
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
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"].clone()
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
                "(async () => {{ await app.workspace.getLeafById({}).setViewState({}); return true; }})()",
                self.leaf,
                json!({"type":view_type,"state":{"file":path,"mode":"preview"}})
            ),
        );
    }
}

impl Drop for DesktopFixture {
    fn drop(&mut self) {
        // Do not panic a second time if the app was closed during a failed test.
        let expression = format!(
            "(() => {{ app.workspace.getLeafById({})?.detach(); for (const leaf of ['markdown'].flatMap(type=>app.workspace.getLeavesOfType(type))) {{ if (leaf.view.file?.path.startsWith({})) leaf.detach(); }} const original = app.workspace.getLeafById({}); if (original) app.workspace.setActiveLeaf(original, {{focus:true}}); return true; }})()",
            self.leaf,
            json!(format!("{}/", self.relative)),
            self.original_leaf
        );
        let _ = std::panic::catch_unwind(|| obsidian_action(&self.vault, &expression));
    }
}

#[test]
#[ignore = "requires TASKCLI_OBSIDIAN_VAULT and enabled TaskNotes/Bases plugins in an open desktop vault"]
fn tasknotes_renders_both_formats_and_resolves_task_note_links() {
    let vault =
        std::env::var("TASKCLI_OBSIDIAN_VAULT").expect("choose the open test vault explicitly");
    assert_eq!(
        obsidian(
            &vault,
            "!!app.plugins.plugins.tasknotes && !!app.internalPlugins.getPluginById('bases')?.enabled && app.plugins.plugins.tasknotes.settings.taskTag === 'task'"
        ),
        true,
        "Enable TaskNotes and Bases and configure the task tag and seven statuses before running this test"
    );
    for format in ["obsidian", "markdown"] {
        exercise_plugin_views(&vault, format);
    }
}

#[allow(clippy::too_many_lines)] // Keep the opt-in desktop scenario and cleanup visible together.
fn exercise_plugin_views(vault: &str, format: &str) {
    let info = obsidian(
        vault,
        "({root:app.vault.adapter.basePath,leaf:app.workspace.getMostRecentLeaf()?.id})",
    );
    let root = PathBuf::from(info["root"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    let parent =
        std::env::var("TASKCLI_OBSIDIAN_PARENT").unwrap_or_else(|_| "00-Inbox/agent".into());
    let parent = root.join(parent).canonicalize().unwrap();
    assert!(
        parent.starts_with(&root),
        "test output must stay inside the vault"
    );
    let output = tempfile::Builder::new()
        .prefix("taskcli-smoke-")
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
    let mut f = DesktopFixture {
        vault: vault.to_owned(),
        original_leaf: info["leaf"].clone(),
        leaf,
        relative,
        output,
        metadata: tempfile::tempdir().unwrap(),
    };
    f.cli(&[
        "init",
        "--format",
        format,
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
    let expression = format!(
        "(() => {{const el=app.workspace.getLeafById({}).view.contentEl;return {{columns:[...el.querySelectorAll('.kanban-view__column')].map(e=>e.dataset.group),cards:[...el.querySelectorAll('.task-card')].map(e=>({{path:e.dataset.taskPath,status:e.dataset.status}}))}};}})()",
        f.leaf
    );
    let rendered = wait_for(&f.vault, &expression, |v| {
        v["cards"].as_array().is_some_and(|a| a.len() == 2)
    });
    assert_eq!(
        rendered["columns"],
        json!([
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
        "(() => {{const el=app.workspace.getLeafById({}).view.contentEl;return [...new Set([...el.querySelectorAll('a.internal-link, .tasknotes-inline-widget[data-task-path]')].map(e=>e.dataset.taskPath??app.metadataCache.getFirstLinkpathDest(decodeURIComponent(e.getAttribute('data-href')??e.getAttribute('href')??''),{})?.path).filter(p=>p?.includes('/Tasks/')))];}})()",
        f.leaf,
        json!(job_path)
    );
    let resolved = wait_for(&f.vault, &links, |v| {
        v.as_array().is_some_and(|a| a.len() == 2)
    });
    assert!(resolved.as_array().unwrap().contains(&json!(path)));
    let task_info = obsidian(
        &f.vault,
        &format!(
            "(async()=>{{const path={};const t=await app.plugins.plugins.tasknotes.cacheManager.getTaskInfo(path);const file=app.vault.getAbstractFileByPath(path);return {{path:t?.path,status:t?.status,id:app.metadataCache.getFileCache(file)?.frontmatter?.id}};}})()",
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
