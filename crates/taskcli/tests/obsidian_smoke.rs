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
    let output = Command::new(std::env::var("OBSIDIAN_BIN").unwrap_or_else(|_| "obsidian".into()))
        .arg(format!("vault={vault}"))
        .arg("eval")
        .arg(format!("code={code}"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
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
            "Obsidian did not reach expected state: {value}"
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

    fn open(&self, path: &str) {
        wait_for(
            &self.vault,
            &format!("!!app.vault.getAbstractFileByPath({})", json!(path)),
            |v| v == true,
        );
        obsidian_action(
            &self.vault,
            &format!(
                "(() => {{ app.workspace.getLeafById({}).setViewState({}); return true; }})()",
                self.leaf,
                json!({"type":"markdown","state":{"file":path,"mode":"preview"}})
            ),
        );
    }
}

impl Drop for DesktopFixture {
    fn drop(&mut self) {
        // Do not panic a second time if the app was closed during a failed test.
        let expression = format!(
            "(() => {{ app.workspace.getLeafById({})?.detach(); for (const leaf of app.workspace.getLeavesOfType('markdown')) {{ if (leaf.view.file?.path.startsWith({})) leaf.detach(); }} const original = app.workspace.getLeafById({}); if (original) app.workspace.setActiveLeaf(original, {{focus:true}}); return true; }})()",
            self.leaf,
            json!(format!("{}/", self.relative)),
            self.original_leaf
        );
        let _ = Command::new(std::env::var("OBSIDIAN_BIN").unwrap_or_else(|_| "obsidian".into()))
            .arg(format!("vault={}", self.vault))
            .arg("eval")
            .arg(format!("code={expression}"))
            .output();
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Keep the opt-in desktop scenario and cleanup visible together.
#[ignore = "requires TASKCLI_OBSIDIAN_VAULT and an open desktop Obsidian instance"]
fn actual_obsidian_renders_seven_columns_and_navigates_plan_and_task_links() {
    let vault =
        std::env::var("TASKCLI_OBSIDIAN_VAULT").expect("choose the open test vault explicitly");
    let info = obsidian(
        &vault,
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
        &vault,
        "app.workspace.setActiveLeaf(app.workspace.getLeaf('tab'),{focus:true})",
    );
    let leaf = obsidian(&vault, "app.workspace.getMostRecentLeaf()?.id");
    assert_ne!(leaf, info["leaf"], "use a dedicated test tab");
    let f = DesktopFixture {
        vault,
        original_leaf: info["leaf"].clone(),
        leaf,
        relative,
        output,
        metadata: tempfile::tempdir().unwrap(),
    };
    f.cli(&[
        "init",
        "--format",
        "obsidian",
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
    f.open(&board);
    let expression = format!(
        "(() => {{ const view=app.workspace.getLeafById({}).view;return {{headers:[...view.contentEl.querySelectorAll('table th')].map(e=>e.textContent.trim()),cells:[...view.contentEl.querySelectorAll('table td')].map(e=>e.textContent),links:[...view.contentEl.querySelectorAll('table a.internal-link')].map(e=>({{text:e.textContent,href:e.getAttribute('data-href')}})),checkboxes:view.contentEl.querySelectorAll('input[type=checkbox]').length}}; }})()",
        f.leaf
    );
    let rendered = wait_for(&f.vault, &expression, |v| {
        v["links"].as_array().is_some_and(|a| a.len() == 2)
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
    assert_eq!(rendered["checkboxes"], 0);
    assert!(
        rendered["cells"].as_array().unwrap().iter().any(|cell| cell
            == "Open Render | Unicode \u{2603} [link] & <tag> [[internal]] *bold* · PLANNING"),
        "{rendered}"
    );
    for (label, expected) in [
        (
            "Open",
            format!("{}/{}", f.relative, plan["path"].as_str().unwrap()),
        ),
        (
            "Task without a Plan",
            format!("{}/{}", f.relative, job["document_path"].as_str().unwrap()),
        ),
    ] {
        f.open(&board);
        let ready = format!(
            "[...app.workspace.getLeafById({}).view.contentEl.querySelectorAll('table a.internal-link')].some(a=>a.textContent==={})",
            f.leaf,
            json!(label)
        );
        wait_for(&f.vault, &ready, |v| v == true);
        obsidian_action(
            &f.vault,
            &format!(
                "(() => {{[...app.workspace.getLeafById({}).view.contentEl.querySelectorAll('table a.internal-link')].find(a=>a.textContent==={}).click();return true;}})()",
                f.leaf,
                json!(label)
            ),
        );
        wait_for(&f.vault, "app.workspace.getActiveFile()?.path", |v| {
            v.as_str() == Some(&expected)
        });
    }
    let anchor = unplanned["id"].as_str().unwrap().replace('_', "-");
    wait_for(
        &f.vault,
        &format!(
            "!!app.metadataCache.getFileCache(app.workspace.getActiveFile())?.blocks?.[{}]",
            json!(anchor)
        ),
        |v| v == true,
    );
    // Keep the TempDir alive through all app reads; Drop restores views first.
    assert!(f.output.path().exists());
}
