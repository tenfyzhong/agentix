use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

struct Cli {
    dir: TempDir,
}
impl Cli {
    fn new(format: &str) -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("vault/.obsidian")).unwrap();
        let cli = Self { dir };
        cli.ok(&[
            "init",
            "--format",
            format,
            "--root",
            cli.dir.path().join("vault").to_str().unwrap(),
            "--directory",
            "Tasks 中文",
            "--database",
            cli.dir.path().join("state.sqlite3").to_str().unwrap(),
        ]);
        cli
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_taskcli"))
            .arg("--config")
            .arg(self.dir.path().join("config.toml"))
            .arg("--json")
            .args(args)
            .current_dir(self.dir.path())
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], true);
        value["result"].clone()
    }
}

#[test]
fn standalone_json_workflow_in_both_document_formats() {
    for format in ["markdown", "obsidian"] {
        let cli = Cli::new(format);
        let project = cli.ok(&[
            "project",
            "register",
            "--name",
            "Demo",
            "--root",
            cli.dir.path().to_str().unwrap(),
        ]);
        let pid = project["id"].as_str().unwrap();
        let job = cli.ok(&[
            "job",
            "create",
            "--project",
            pid,
            "--title",
            "Deliver task board",
            "--goal",
            "CLI works standalone",
        ]);
        let jid = job["id"].as_str().unwrap();
        let task = cli.ok(&["task", "add", "--job", jid, "--title", "Build"]);
        let tid = task["id"].as_str().unwrap();
        cli.ok(&["plan", "create", tid, "--body", "# Build\nRun tests."]);
        let claim = cli.ok(&[
            "task",
            "claim",
            tid,
            "--executor",
            "agent:test",
            "--session",
            "session:test",
            "--delegated-by",
            "team:test",
        ]);
        let token = claim["lease"]["token"].as_str().unwrap();
        let context = cli.ok(&["context", "--session", "session:test"]);
        assert_eq!(context["job_id"], jid);
        assert_eq!(context["documents"]["format"], format);
        cli.ok(&[
            "task",
            "done",
            tid,
            "--session",
            "session:test",
            "--lease-token",
            token,
        ]);
        assert_eq!(cli.ok(&["job", "show", jid])["status"], "COMPLETED");
        cli.ok(&["job", "archive", jid]);
        assert_eq!(
            cli.ok(&["job", "list", "--archived"])
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            !cli.ok(&["event", "list", "--job", jid])["events"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(cli.ok(&["doctor"])["healthy"], true);
        cli.ok(&["sync"]);
    }
}

#[test]
fn invalid_arguments_and_business_errors_have_distinct_exit_codes() {
    let cli = Cli::new("markdown");
    assert_eq!(cli.run(&["watch"]).status.code(), Some(2));
    let out = cli.run(&["task", "show", "task_missing"]);
    assert_eq!(out.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert!(!value["error"]["message"].as_str().unwrap().is_empty());
    let version = Command::new(env!("CARGO_BIN_EXE_taskcli"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("taskcli "));
}

#[test]
fn hooks_consume_host_json_and_do_not_require_a_job() {
    use std::io::Write;
    let cli = Cli::new("markdown");
    let mut child = Command::new(env!("CARGO_BIN_EXE_taskcli"))
        .arg("--config")
        .arg(cli.dir.path().join("config.toml"))
        .args(["--json", "hook", "session-start"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(json!({"session_id":"host-session","cwd":cli.dir.path(),"hook_event_name":"SessionStart"}).to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["session_ref"], "host-session");
}

#[test]
fn git_worktrees_share_one_project() {
    let cli = Cli::new("markdown");
    let repo = cli.dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-s",
            "--no-verify",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let worktree = cli.dir.path().join("branch");
    assert!(
        Command::new("git")
            .args(["worktree", "add", "-b", "test"])
            .arg(&worktree)
            .current_dir(&repo)
            .output()
            .unwrap()
            .status
            .success()
    );
    let first = cli.ok(&["project", "register", "--root", repo.to_str().unwrap()]);
    let second = cli.ok(&["project", "register", "--root", worktree.to_str().unwrap()]);
    assert_eq!(first["id"], second["id"]);
    assert_eq!(cli.ok(&["project", "list"]).as_array().unwrap().len(), 1);
}
