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
                .arg(format!("code=JSON.stringify(({expression}) ?? null)"))
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
        // A fresh owned tab avoids Kanban reusing a disposed view after link navigation.
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
            "(() => {{ app.workspace.getLeafById({})?.detach(); for (const leaf of ['markdown','kanban'].flatMap(type=>app.workspace.getLeavesOfType(type))) {{ if (leaf.view.file?.path.startsWith({})) leaf.detach(); }} const original = app.workspace.getLeafById({}); if (original) app.workspace.setActiveLeaf(original, {{focus:true}}); return true; }})()",
            self.leaf,
            json!(format!("{}/", self.relative)),
            self.original_leaf
        );
        let _ = std::panic::catch_unwind(|| obsidian_action(&self.vault, &expression));
    }
}

#[test]
#[ignore = "requires TASKCLI_OBSIDIAN_VAULT and enabled Tasks/Kanban plugins in an open desktop vault"]
fn actual_plugins_render_both_formats_and_navigate_plan_and_task_links() {
    let vault =
        std::env::var("TASKCLI_OBSIDIAN_VAULT").expect("choose the open test vault explicitly");
    assert_eq!(
        obsidian(
            &vault,
            "!!app.plugins.plugins['obsidian-kanban'] && !!app.plugins.plugins['obsidian-tasks-plugin']"
        ),
        true,
        "Install and enable Kanban and Tasks in the chosen test vault before running this test"
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
    let unplanned = f.cli(&[
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
    f.open(&board, "kanban");
    let expression = format!(
        "(() => {{ const view=app.workspace.getLeafById({}).view;return {{type:view.getViewType(),headers:[...view.contentEl.querySelectorAll('.kanban-plugin__lane-title-text')].map(e=>e.textContent.trim()),cells:[...view.contentEl.querySelectorAll('.kanban-plugin__item-title')].map(e=>e.textContent),links:[...view.contentEl.querySelectorAll('.kanban-plugin__item-title a.internal-link')].map(e=>({{text:e.textContent,href:e.getAttribute('data-href')}})),checkboxes:view.contentEl.querySelectorAll('.kanban-plugin__item input[type=checkbox]').length}}; }})()",
        f.leaf
    );
    let rendered = wait_for(&f.vault, &expression, |v| {
        v["links"].as_array().is_some_and(|a| !a.is_empty())
    });
    assert_eq!(
        rendered["headers"],
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
    assert_eq!(rendered["type"], "kanban");
    assert_eq!(rendered["checkboxes"], 0);
    assert!(
        rendered["cells"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cell| cell
                .as_str()
                .is_some_and(|text| text.contains(task["name"].as_str().unwrap())
                    && text.contains("PLANNING"))),
        "{rendered}"
    );
    let tasks = board.replace("/Board.md", "/Tasks.md");
    f.open(&tasks, "markdown");
    let query_results = format!(
        "(() => {{ const el=app.workspace.getLeafById({}).view.contentEl; return {{count:el.querySelectorAll('.task-list-item-checkbox').length,text:el.textContent}}; }})()",
        f.leaf
    );
    let results = wait_for(&f.vault, &query_results, |v| v["count"] == 2);
    assert!(
        results["text"]
            .as_str()
            .unwrap()
            .contains("Task without a Plan")
    );
    assert!(results["text"].as_str().unwrap().contains("PLANNING"));
    for (label, expected) in [
        (
            "Plan",
            format!("{}/{}", f.relative, plan["path"].as_str().unwrap()),
        ),
        (
            "Job",
            format!("{}/{}", f.relative, job["document_path"].as_str().unwrap()),
        ),
    ] {
        if format == "markdown" && label == "Plan" {
            f.open(&board, "kanban");
            let embedded = format!(
                "[...app.workspace.getLeafById({}).view.contentEl.querySelectorAll('.internal-embed')].some(e=>e.getAttribute('src')?.includes('Plans/'))",
                f.leaf
            );
            wait_for(&f.vault, &embedded, |v| v == true);
            continue;
        }
        f.open(&board, "kanban");
        let ready = format!(
            "(() => {{const v=app.workspace.getLeafById({}).view;return {{type:v.getViewType(),file:v.file?.path,links:[...v.contentEl.querySelectorAll('.kanban-plugin__item-title a.internal-link')].map(a=>a.textContent)}};}})()",
            f.leaf
        );
        wait_for(&f.vault, &ready, |v| {
            v["links"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|s| s.as_str().is_some_and(|s| s.starts_with(label)))
            })
        });
        obsidian_action(
            &f.vault,
            &format!(
                "(() => {{[...app.workspace.getLeafById({}).view.contentEl.querySelectorAll('.kanban-plugin__item-title a.internal-link')].find(a=>a.textContent.startsWith({})).click();return true;}})()",
                f.leaf,
                json!(label)
            ),
        );
        wait_for(&f.vault, "app.workspace.getActiveFile()?.path", |v| {
            v.as_str() == Some(&expected)
        });
    }
    if format == "obsidian" {
        let anchor = unplanned["id"].as_str().unwrap().replace('_', "-");
        wait_for(
            &f.vault,
            &format!(
                "!!app.metadataCache.getFileCache(app.workspace.getActiveFile())?.blocks?.[{}]",
                json!(anchor)
            ),
            |v| v == true,
        );
    }
    // Keep the TempDir alive through all app reads; Drop restores views first.
    assert!(f.output.path().exists());
}
