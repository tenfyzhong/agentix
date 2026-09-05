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
            "Tasks \u{2603}",
            "--database",
            cli.dir.path().join("state.sqlite3").to_str().unwrap(),
        ]);
        cli
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_taskcli"));
        command
            .arg("--config")
            .arg(self.dir.path().join("config.toml"))
            .arg("--json")
            .args(args)
            .current_dir(self.dir.path());
        command
    }
    fn job(&self, title: &str) -> String {
        let project = self.ok(&[
            "project",
            "register",
            "--name",
            "Demo",
            "--root",
            self.dir.path().to_str().unwrap(),
        ]);
        self.ok(&[
            "job",
            "create",
            "--project",
            project["id"].as_str().unwrap(),
            "--title",
            title,
        ])["id"]
            .as_str()
            .unwrap()
            .into()
    }
    fn task(&self, job: &str, title: &str) -> String {
        let task = self.ok(&["task", "add", "--job", job, "--title", title]);
        let id = task["id"].as_str().unwrap();
        id.into()
    }
    fn owned(&self, args: &[&str], claim: &Value) -> Value {
        let mut args = args.to_vec();
        args.extend([
            "--session",
            claim["lease"]["session_ref"].as_str().unwrap(),
            "--lease-token",
            claim["lease"]["token"].as_str().unwrap(),
        ]);
        self.ok(&args)
    }
    fn claim(&self, task: &str, session: &str) -> Value {
        self.ok(&[
            "task",
            "claim",
            task,
            "--session",
            session,
            "--executor",
            &format!("agent:{session}"),
        ])
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
fn cli_claim_plan_start_done_requires_ownership_and_preserves_lease() {
    let cli = Cli::new("markdown");
    let job = cli.job("Owned planning");
    let id = cli.task(&job, "Plan safely");
    assert_eq!(
        cli.run(&["plan", "create", &id, "--body", "# Plan"])
            .status
            .code(),
        Some(1)
    );
    let claim = cli.ok(&[
        "task",
        "claim",
        &id,
        "--executor",
        "agent:a",
        "--session",
        "a",
    ]);
    assert_eq!(claim["phase"], "PLANNING");
    let token = claim["lease"]["token"].as_str().unwrap();
    cli.ok(&[
        "plan",
        "create",
        &id,
        "--body",
        "# Plan",
        "--session",
        "a",
        "--lease-token",
        token,
    ]);
    assert_eq!(
        cli.run(&[
            "task",
            "done",
            &id,
            "--session",
            "a",
            "--lease-token",
            token
        ])
        .status
        .code(),
        Some(1)
    );
    assert_eq!(
        cli.run(&[
            "task",
            "start",
            &id,
            "--session",
            "b",
            "--lease-token",
            token
        ])
        .status
        .code(),
        Some(1)
    );
    let args = [
        "task",
        "start",
        &id,
        "--session",
        "a",
        "--lease-token",
        token,
        "--idempotency-key",
        "start-once",
    ];
    let started = cli.ok(&args);
    assert_eq!(started["phase"], "EXECUTING");
    assert_eq!(started["lease"]["token"], token);
    assert_eq!(cli.ok(&args), started);
    assert_eq!(
        cli.ok(&["context", "--session", "a"])["task"]["phase"],
        "EXECUTING"
    );
    cli.ok(&[
        "task",
        "done",
        &id,
        "--session",
        "a",
        "--lease-token",
        token,
    ]);
}

struct RunningCli(std::process::Child);
impl RunningCli {
    fn start(cli: &Cli, args: &[&str]) -> Self {
        Self(
            cli.command(args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }
    fn output(&mut self) -> Output {
        use std::io::Read;
        let status = self.0.wait().unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        self.0
            .stdout
            .take()
            .unwrap()
            .read_to_end(&mut stdout)
            .unwrap();
        self.0
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        Output {
            status,
            stdout,
            stderr,
        }
    }
}
impl Drop for RunningCli {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn separate_cli_processes_racing_to_claim_have_exactly_one_winner() {
    let cli = Cli::new("markdown");
    let job = cli.job("Race");
    let task = cli.task(&job, "Exclusive");
    let mut children: Vec<_> = (0..8)
        .map(|i| {
            RunningCli::start(
                &cli,
                &[
                    "task",
                    "claim",
                    &task,
                    "--executor",
                    &format!("agent:{i}"),
                    "--session",
                    &format!("session:{i}"),
                ],
            )
        })
        .collect();
    let outputs: Vec<_> = children.iter_mut().map(RunningCli::output).collect();
    assert_eq!(outputs.iter().filter(|o| o.status.success()).count(), 1);
    for output in outputs.iter().filter(|o| !o.status.success()) {
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("conflict"),
            "{value}"
        );
    }
    assert_eq!(cli.ok(&["task", "show", &task])["status"], "IN_PROGRESS");
}

#[test]
fn concurrent_cli_jobs_preserve_notes_and_all_projections() {
    for format in ["markdown", "obsidian"] {
        let cli = Cli::new(format);
        let jobs: Vec<_> = (0..4)
            .map(|i| cli.job(&format!("Requirement {i}")))
            .collect();
        let tasks: Vec<_> = jobs
            .iter()
            .enumerate()
            .map(|(i, j)| cli.task(j, &format!("Parallel {i}")))
            .collect();
        let output = cli.dir.path().join("vault/Tasks \u{2603}");
        let paths: Vec<_> = jobs
            .iter()
            .map(|j| {
                output.join(
                    cli.ok(&["job", "show", j])["document_path"]
                        .as_str()
                        .unwrap(),
                )
            })
            .collect();
        for path in &paths {
            let body = std::fs::read_to_string(path).unwrap().replace(
                "<!-- taskcli:notes:start -->",
                "<!-- taskcli:notes:start -->\nKeep my notes.",
            );
            std::fs::write(path, body).unwrap();
        }
        let mut children: Vec<_> = tasks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                RunningCli::start(
                    &cli,
                    &[
                        "task",
                        "claim",
                        t,
                        "--executor",
                        &format!("agent:{i}"),
                        "--session",
                        &format!("session:{i}"),
                    ],
                )
            })
            .collect();
        for child in &mut children {
            let result = child.output();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stdout)
            );
            let value: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert!(value["projection_pending"].is_null(), "{value}");
        }
        for (path, task) in paths.iter().zip(&tasks) {
            let body = std::fs::read_to_string(path).unwrap();
            assert_eq!(body.matches("Keep my notes.").count(), 1);
            assert!(body.contains("IN_PROGRESS") && body.contains(task));
        }
        assert_eq!(
            cli.ok(&["task", "list", "--status", "IN_PROGRESS"])
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(cli.ok(&["doctor"])["healthy"], true);
    }
}

#[tokio::test]
async fn killed_cli_after_database_commit_replays_without_duplicates_and_repairs_files() {
    let cli = Cli::new("markdown");
    let job = cli.job("Crash recovery");
    let store = agentix_task::Store::open(&cli.dir.path().join("state.sqlite3"))
        .await
        .unwrap();
    let lock =
        std::fs::File::open(cli.dir.path().join("vault/Tasks \u{2603}/.taskcli.lock")).unwrap();
    lock.lock().unwrap();
    let args = [
        "task",
        "add",
        "--job",
        &job,
        "--title",
        "Committed before crash",
        "--idempotency-key",
        "crash-once",
    ];
    let mut child = RunningCli::start(&cli, &args);
    let task = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let state = store.snapshot().await.unwrap();
            if let Some(task) = state.tasks.first() {
                break task.clone();
            }
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "CLI exited before commit"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "projection must still be blocked"
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let sequence = store.latest_sequence().await.unwrap();
    drop(lock);
    drop(store);
    let replay = cli.ok(&args);
    assert_eq!(replay["id"], task.id);
    let reopened = agentix_task::Store::open(&cli.dir.path().join("state.sqlite3"))
        .await
        .unwrap();
    assert_eq!(reopened.latest_sequence().await.unwrap(), sequence);
    assert_eq!(reopened.snapshot().await.unwrap().tasks.len(), 1);
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
    let path = cli.ok(&["job", "show", &job])["document_path"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        std::fs::read_to_string(cli.dir.path().join("vault/Tasks \u{2603}").join(path))
            .unwrap()
            .contains(&task.id)
    );
}

#[test]
fn plugin_entrypoints_execute_the_compiled_taskcli() {
    let plugin =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/agent-task-manager");
    let output = Command::new("node")
        .args(["--test", "tests/integration.mjs"])
        .current_dir(plugin)
        .env("TASKCLI_BIN", env!("CARGO_BIN_EXE_taskcli"))
        .output()
        .expect("Node.js 24+ is required for plugin integration tests");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_rejects_invalid_inputs_and_preserves_configuration_and_state() {
    let cli = Cli::new("markdown");
    let job = cli.job("Validation");
    let task = cli.task(&job, "Check inputs");
    let before = cli.ok(&["task", "show", &task]);
    for args in [
        vec!["task", "update", &task, "--title", " "],
        vec!["task", "update", &task, "--position=-1"],
        vec!["task", "list", "--status", "UNKNOWN"],
        vec!["task", "depend", &task, &task],
        vec!["task", "claim", &task],
        vec!["job", "archive", &job],
        vec!["job", "unarchive", &job],
        vec!["job", "list", "--period", "2026-13"],
        vec!["job", "list", "--created-from", "not-a-date"],
        vec!["event", "list", "--after=-1"],
        vec!["event", "list", "--limit", "0"],
        vec!["event", "list", "--limit", "1001"],
        vec!["plan", "create", &task, "--body", "duplicate"],
    ] {
        let output = cli.run(&args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["ok"], false);
        assert_eq!(cli.ok(&["task", "show", &task]), before);
    }
    for args in [
        vec!["task", "block", &task],
        vec!["job", "list", "--active", "--completed"],
        vec!["plan", "revise", &task, "--body", "x", "--file", "x"],
    ] {
        assert_eq!(cli.run(&args).status.code(), Some(2), "{args:?}");
    }
    let config = std::fs::read(cli.dir.path().join("config.toml")).unwrap();
    assert_eq!(
        cli.run(&[
            "init",
            "--format",
            "markdown",
            "--root",
            cli.dir.path().join("vault").to_str().unwrap()
        ])
        .status
        .code(),
        Some(1)
    );
    assert_eq!(
        std::fs::read(cli.dir.path().join("config.toml")).unwrap(),
        config
    );
}

#[test]
fn cli_dependency_edits_filters_plan_files_and_archive_round_trip() {
    let cli = Cli::new("markdown");
    let job = cli.job("First requirement");
    let a = cli.task(&job, "prerequisite");
    let b = cli.task(&job, "dependent");
    cli.ok(&["task", "depend", &b, &a]);
    assert_eq!(
        cli.ok(&["task", "list", "--ready"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    cli.ok(&["task", "undepend", &b, &a]);
    assert_eq!(
        cli.ok(&["task", "list", "--ready"])
            .as_array()
            .unwrap()
            .len(),
        2
    );
    cli.ok(&[
        "task",
        "update",
        &b,
        "--title",
        "Changed title",
        "--position",
        "0",
    ]);
    cli.ok(&[
        "job",
        "update",
        &job,
        "--title",
        "Changed job",
        "--goal",
        "Acceptance goal",
    ]);
    let plan = cli.dir.path().join("input-plan.md");
    std::fs::write(&plan, "# File plan\nPreserve exact body.\n").unwrap();
    let claim = cli.claim(&b, "editor");
    cli.owned(&["plan", "create", &b, "--body", "# Initial"], &claim);
    cli.owned(
        &["plan", "revise", &b, "--file", plan.to_str().unwrap()],
        &claim,
    );
    assert_eq!(
        cli.ok(&["plan", "show", &b])["body"],
        "# File plan\nPreserve exact body.\n"
    );
    cli.owned(&["task", "release", &b, "--reason", "cancel scope"], &claim);
    cli.ok(&["job", "cancel", &job]);
    assert!(
        cli.ok(&["task", "list", "--job", &job])
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["status"] == "CANCELLED")
    );
    cli.ok(&["job", "archive", &job]);
    assert_eq!(
        cli.ok(&["job", "list", "--archived"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    cli.ok(&["job", "unarchive", &job]);
    assert!(
        cli.ok(&["job", "list", "--archived"])
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(cli.ok(&["job", "show", &job])["status"], "CANCELLED");
    let next = cli.job("New scope");
    assert_ne!(next, job);
    let events = cli.ok(&["event", "list", "--job", &job, "--limit", "2"]);
    assert_eq!(events["events"].as_array().unwrap().len(), 2);
    let cursor = events["next_cursor"].as_i64().unwrap();
    let page = cli.ok(&[
        "event",
        "list",
        "--job",
        &job,
        "--after",
        &cursor.to_string(),
    ]);
    assert!(
        page["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["sequence"].as_i64().unwrap() > cursor)
    );
}

#[test]
fn inline_plan_bodies_accept_yaml_frontmatter_and_preserve_it_verbatim() {
    let cli = Cli::new("obsidian");
    let job = cli.job("Frontmatter");
    let task = cli.ok(&[
        "task",
        "add",
        "--job",
        &job,
        "--title",
        "Plan with properties",
    ]);
    let id = task["id"].as_str().unwrap();
    let claim = cli.ok(&[
        "task",
        "claim",
        id,
        "--session",
        "frontmatter",
        "--executor",
        "agent:frontmatter",
    ]);
    let token = claim["lease"]["token"].as_str().unwrap();
    for (command, title) in [("create", "First"), ("revise", "Second")] {
        let body = format!("---\ntitle: {title}\n---\n\n# Plan\n");
        cli.ok(&[
            "plan",
            command,
            id,
            "--body",
            &body,
            "--session",
            "frontmatter",
            "--lease-token",
            token,
        ]);
        assert_eq!(cli.ok(&["plan", "show", id])["body"], body);
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
        cli.ok(&[
            "plan",
            "create",
            tid,
            "--body",
            "# Build\nRun tests.",
            "--session",
            "session:test",
            "--lease-token",
            token,
        ]);
        cli.ok(&[
            "task",
            "start",
            tid,
            "--session",
            "session:test",
            "--lease-token",
            token,
        ]);
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
