use super::*;
use std::{fs, path::Path};

fn properties(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap();
    serde_yaml::from_str(
        text.strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0,
    )
    .unwrap()
}

fn job_path(cli: &Cli, job: &str) -> std::path::PathBuf {
    cli.dir.path().join("vault/Tasks ☃").join(
        cli.ok(&["job", "show", job])["document_path"]
            .as_str()
            .unwrap(),
    )
}

#[test]
fn agent_session_creation_is_persisted_and_job_keeps_its_creator() {
    let cli = Cli::new();
    let seed = cli.job("Seed");
    let project = cli.ok(&["job", "show", &seed])["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for host in ["codex", "claude", "pi", "omp"] {
        let session = format!("{host}-session");
        let executor = format!("agent:{host}:{session}");
        let job = cli.ok(&[
            "job",
            "create",
            "--project",
            &project,
            "--title",
            host,
            "--executor",
            &executor,
            "--session",
            &session,
        ]);
        let job_id = job["id"].as_str().unwrap();
        let job_file = job_path(&cli, job_id);
        let task = cli.ok(&[
            "task",
            "add",
            "--job",
            job_id,
            "--title",
            host,
            "--executor",
            &executor,
            "--session",
            &session,
        ]);
        let task_id = task["id"].as_str().unwrap();
        let task_file = fs::read_dir(cli.dir.path().join("vault/Tasks ☃/Projects/Demo/Tasks"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| properties(path)["task_id"] == task_id)
            .unwrap();
        cli.ok(&["sync"]);
        for path in [&job_file, &task_file] {
            let props = properties(path);
            assert_eq!(props["agent"], host);
            assert_eq!(props["session_id"], session);
        }
        cli.claim(task_id, "codex");
        assert_eq!(properties(&task_file)["session_id"], "codex");
        cli.ok(&["hook", "session-end", "--session", "codex"]);
        let props = properties(&job_file);
        assert_eq!(props["agent"], host);
        assert_eq!(props["session_id"], session);
    }
}

#[test]
fn legacy_dashboard_migration_preserves_conflicts_and_recovers_in_a_new_process() {
    let cli = Cli::new();
    cli.job("Migration");
    let root = cli.dir.path().join("vault/Tasks ☃");
    let markdown = "---\nid: dashboard\ntaskix-generated: true\ntags: [agent/dashboard]\n---\n# Task dashboard\n";
    fs::write(root.join("Dashboard.md"), markdown).unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let store = agentix_task::Store::open(&cli.dir.path().join("state.sqlite3"))
            .await
            .unwrap();
        let mut paths = store.metadata("documents").await.unwrap().unwrap();
        paths["dashboard"] = json!("Dashboard.md");
        store.set_metadata("documents", &paths).await.unwrap();
    });
    let base = root.join("Dashboard.base");
    fs::write(&base, "# Keep this personal Base\nviews: []\n").unwrap();
    let result = cli.run(&["sync"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stdout).contains("unmanaged document"));
    assert_eq!(
        fs::read_to_string(&base).unwrap(),
        "# Keep this personal Base\nviews: []\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("Dashboard.md")).unwrap(),
        markdown
    );
    fs::remove_file(&base).unwrap();
    cli.ok(&["sync"]);
    assert!(!root.join("Dashboard.md").exists());
    let base_text = fs::read_to_string(&base).unwrap();
    let parsed: Value = serde_yaml::from_str(&base_text).unwrap();
    assert_eq!(
        parsed["formulas"]["name"],
        "link(file.path, if(note.name, note.name, note.title))"
    );
    assert_eq!(
        parsed["views"][0]["order"],
        json!(["formula.name", "formula.status", "formula.updated"])
    );
    let updated = properties(&root.join("Projects/Demo/Board.md"))["updated_at"].clone();
    let modified = base.metadata().unwrap().modified().unwrap();
    cli.ok(&["sync"]);
    assert_eq!(base.metadata().unwrap().modified().unwrap(), modified);
    assert_eq!(
        properties(&root.join("Projects/Demo/Board.md"))["updated_at"],
        updated
    );
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
}

#[test]
#[ignore = "requires TASKIX_RELATIVE_PATHS_BACKUP with an offline task database and vault"]
fn real_backup_relative_paths_survive_two_vault_moves() {
    let backup = std::path::PathBuf::from(
        std::env::var("TASKIX_RELATIVE_PATHS_BACKUP").expect("offline backup directory"),
    );
    let cli = Cli::new();
    let database = cli.dir.path().join("restored.sqlite3");
    fs::copy(backup.join("tasks.sqlite3"), &database).unwrap();
    copy_offline_vault(&backup.join("vault"), &cli.dir.path().join("vault"));
    let documents: Value =
        serde_json::from_str(&fs::read_to_string(backup.join("documents.json")).unwrap()).unwrap();
    let config_path = cli.dir.path().join("config.toml");
    let mut config: toml::Value =
        toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config["storage"]["path"] = toml::Value::String(database.to_str().unwrap().to_owned());
    config["documents"]["directory"] =
        toml::Value::String(documents["directory"].as_str().unwrap().to_owned());
    config["documents"]["archive_directory"] =
        toml::Value::String(documents["archive_directory"].as_str().unwrap().to_owned());
    fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    cli.ok(&["sync"]);
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let snapshot = || {
        runtime.block_on(async {
            let store = agentix_task::Store::open_read_only(&database)
                .await
                .unwrap();
            serde_json::to_value(store.snapshot().await.unwrap()).unwrap()
        })
    };
    let before = snapshot();
    for name in ["moved-backup-once", "moved-backup-twice"] {
        let old = Path::new(config["documents"]["root"].as_str().unwrap());
        let root = cli.dir.path().join(name);
        fs::rename(old, &root).unwrap();
        config["documents"]["root"] = toml::Value::String(root.to_str().unwrap().to_owned());
        fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
        cli.ok(&["sync"]);
        assert_eq!(cli.ok(&["doctor"])["healthy"], true);
        let after = snapshot();
        for kind in ["projects", "jobs", "plans"] {
            let field = match kind {
                "projects" => "document_directory",
                "jobs" => "document_path",
                _ => "path",
            };
            assert_eq!(
                before[kind]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| (&e["id"], &e[field]))
                    .collect::<Vec<_>>(),
                after[kind]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| (&e["id"], &e[field]))
                    .collect::<Vec<_>>()
            );
        }
    }
}

fn copy_offline_vault(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_offline_vault(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn cli_doctor_resolves_relative_archived_plans_after_two_vault_moves() {
    let cli = Cli::new();
    let job = cli.job("Relative archive");
    let task = cli.task(&job, "Keep plan");
    let claim = cli.claim(&task, "relative-archive");
    cli.owned(
        &["plan", "create", &task, "--body", "# Preserve body"],
        &claim,
    );
    cli.owned(&["task", "start", &task], &claim);
    cli.owned(&["task", "done", &task], &claim);
    cli.ok(&["job", "approve", &job]);
    let project = cli.ok(&["job", "show", &job])["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    cli.ok(&["project", "archive", &project]);
    cli.ok(&["sync"]);
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
    let config_path = cli.dir.path().join("config.toml");
    let mut config: toml::Value =
        toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    for directory in ["moved-once", "moved-twice"] {
        let old = Path::new(config["documents"]["root"].as_str().unwrap());
        let root = cli.dir.path().join(directory);
        fs::rename(old, &root).unwrap();
        config["documents"]["root"] = toml::Value::String(root.to_str().unwrap().to_owned());
        fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
        cli.ok(&["sync"]);
        assert_eq!(cli.ok(&["doctor"])["healthy"], true);
        let plan = cli.ok(&["plan", "show", &task]);
        assert!(
            plan["path"]
                .as_str()
                .unwrap()
                .starts_with("Archived Projects/")
        );
        assert!(
            plan["absolute_path"]
                .as_str()
                .unwrap()
                .starts_with(root.to_str().unwrap())
        );
        assert!(plan["body"].as_str().unwrap().contains("# Preserve body"));
    }
}

#[test]
fn cli_project_archive_restores_dashboard_and_task_visibility_in_obsidian() {
    let cli = Cli::new();
    let job = cli.job("Archive project");
    let task = cli.task(&job, "Keep task");
    let claim = cli.claim(&task, "archive");
    let plan = cli.owned(
        &["plan", "create", &task, "--body", "# Keep authored text"],
        &claim,
    );
    cli.owned(&["task", "start", &task], &claim);
    cli.owned(&["task", "done", &task], &claim);
    let project = cli.ok(&["job", "show", &job])["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let root = cli.dir.path().join("vault/Tasks ☃");
    let board = root.join("Projects/Demo/Board.md");
    let note = Path::new(plan["absolute_path"].as_str().unwrap());
    cli.ok(&["job", "approve", &job]);
    cli.ok(&["project", "archive", &project]);
    let archived_board = cli.dir.path().join("vault/Archived Projects/Demo/Board.md");
    let archived_note = archived_board
        .parent()
        .unwrap()
        .join("Tasks")
        .join(note.file_name().unwrap());
    assert!(!board.exists());
    assert_eq!(properties(&archived_board)["status"], "ARCHIVED");
    assert_eq!(properties(&archived_note)["archived"], true);
    assert!(
        properties(&archived_note)["tags"]
            .as_array()
            .unwrap()
            .contains(&json!("archived"))
    );
    assert!(cli.ok(&["project", "list"]).as_array().unwrap().is_empty());
    let base: Value =
        serde_yaml::from_str(&fs::read_to_string(root.join("Dashboard.base")).unwrap()).unwrap();
    assert!(
        base["views"][0]["filters"]["and"]
            .as_array()
            .unwrap()
            .contains(&json!("note.status == \"ACTIVE\""))
    );
    cli.ok(&["project", "unarchive", &project]);
    assert_eq!(properties(&board)["status"], "ACTIVE");
    assert_eq!(properties(note)["archived"], false);
    assert!(
        !properties(note)["tags"]
            .as_array()
            .unwrap()
            .contains(&json!("archived"))
    );
    assert_eq!(
        cli.ok(&["plan", "show", &task])["body"],
        "# Keep authored text"
    );
    assert_eq!(cli.ok(&["project", "list"]).as_array().unwrap().len(), 1);
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
}

#[test]
fn cli_dependency_graph_and_task_notes_follow_cross_job_changes() {
    let cli = Cli::new();
    let upstream = cli.job("Upstream");
    let downstream = cli.job("Downstream");
    let a = cli.task(&upstream, "Design");
    let b = cli.task(&downstream, "Build");
    let c = cli.task(&downstream, "Review");
    cli.ok(&["task", "depend", &b, &a]);
    cli.ok(&["task", "depend", &c, &a]);
    let path = job_path(&cli, &downstream);
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches(&format!("{a}[\"")).count(), 1);
    assert!(text.contains(&format!("{a} --> {b}")));
    assert!(text.contains(&format!("{a} --> {c}")));
    assert!(!text.contains("Dependencies:"));
    let blocked = cli.claim(&b, "dependent");
    let plan = cli.owned(
        &["plan", "create", &b, "--body", "# Wait for design"],
        &blocked,
    );
    assert_eq!(
        properties(Path::new(plan["absolute_path"].as_str().unwrap()))["dependencies"],
        json!([a])
    );
    let failure = cli.run(&[
        "task",
        "start",
        &b,
        "--session",
        "dependent",
        "--lease-token",
        blocked["lease"]["token"].as_str().unwrap(),
    ]);
    assert_eq!(failure.status.code(), Some(1));
    assert_eq!(cli.ok(&["task", "show", &b])["phase"], "PLANNING");
    let claim = cli.claim(&a, "designer");
    cli.owned(&["plan", "create", &a, "--body", "# Design"], &claim);
    cli.owned(&["task", "start", &a], &claim);
    cli.owned(&["task", "done", &a], &claim);
    cli.ok(&["task", "update", &a, "--name", "Approved design"]);
    cli.ok(&["job", "approve", &upstream]);
    cli.ok(&["job", "archive", &upstream]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("Approved design (Job: Upstream) · DONE"));
    assert!(text.contains(&format!("{a} --> {b}")));
    assert_eq!(text.matches(&format!("{a}[\"")).count(), 1);
    assert!(text.contains("Approved design.md"));
    assert_eq!(
        cli.run(&["job", "delete", &upstream]).status.code(),
        Some(1)
    );
    assert!(job_path(&cli, &upstream).exists());
    cli.owned(&["task", "start", &b], &blocked);
    cli.owned(&["task", "done", &b], &blocked);
    cli.ok(&["task", "undepend", &c, &a]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains(&format!("{a} --> {c}")));
    assert!(text.contains(&format!("{a} --> {b}")));
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
}

#[test]
fn cli_projects_every_status_to_mermaid_and_native_boards() {
    let colors = [
        ("TODO", "#cbd5e1"),
        ("IN_PROGRESS", "#bfdbfe"),
        ("BLOCKED", "#fed7aa"),
        ("WAITING_USER", "#ddd6fe"),
        ("DONE", "#bbf7d0"),
        ("FAILED", "#fecaca"),
        ("CANCELLED", "#e2d7e7"),
    ];
    let cli = Cli::new();
    let job = cli.job("Every state");
    for (status, color) in colors {
        let task = cli.task(&job, status);
        let claim = cli.claim(&task, status);
        let plan = cli.owned(
            &["plan", "create", &task, "--body", "# State coverage"],
            &claim,
        );
        match status {
            "IN_PROGRESS" => {
                cli.owned(&["task", "start", &task], &claim);
            }
            "DONE" => {
                cli.owned(&["task", "start", &task], &claim);
                cli.owned(&["task", "done", &task], &claim);
            }
            "CANCELLED" => {
                cli.owned(&["task", "cancel", &task], &claim);
            }
            "TODO" => {
                cli.owned(&["task", "fail", &task, "--reason", "Retry later"], &claim);
                cli.ok(&["task", "retry", &task]);
            }
            other => {
                let command = match other {
                    "BLOCKED" => "block",
                    "WAITING_USER" => "wait",
                    "FAILED" => "fail",
                    _ => panic!("unexpected status {other}"),
                };
                cli.owned(
                    &["task", command, &task, "--reason", "State coverage"],
                    &claim,
                );
            }
        }
        let note = properties(Path::new(plan["absolute_path"].as_str().unwrap()));
        assert_eq!(note["status"], status);
        for tag in ["task", "agent/task"] {
            assert!(note["tags"].as_array().unwrap().contains(&json!(tag)));
        }
        let graph = fs::read_to_string(job_path(&cli, &job)).unwrap();
        assert!(graph.contains(&format!("{status} · {status}")));
        assert!(graph.contains(&format!(":::status_{status}")));
        assert!(graph.contains(&format!(
            "classDef status_{status} fill:{color},stroke:{color},color:#1f2937"
        )));
    }
    assert_eq!(
        cli.ok(&["task", "list", "--job", &job])
            .as_array()
            .unwrap()
            .len(),
        7
    );
}
