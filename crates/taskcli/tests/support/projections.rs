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
fn dashboard_format_switch_preserves_conflicts_and_recovers_in_a_new_process() {
    let cli = Cli::new("markdown");
    cli.job("Migration");
    let root = cli.dir.path().join("vault/Tasks ☃");
    let markdown = fs::read_to_string(root.join("Dashboard.md")).unwrap();
    assert!(markdown.contains("| [Demo](Projects/Demo/Board.md) | ACTIVE |"));
    fs::create_dir(cli.dir.path().join("vault/.obsidian")).unwrap();
    let config_path = cli.dir.path().join("config.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(
        &config_path,
        config.replace("format = \"markdown\"", "format = \"obsidian\""),
    )
    .unwrap();
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
    assert_eq!(parsed["formulas"]["name"], "link(file.path, note.name)");
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
    fs::write(config_path, config).unwrap();
    cli.ok(&["sync"]);
    assert!(!base.exists());
    assert_eq!(
        fs::read_to_string(root.join("Dashboard.md")).unwrap(),
        markdown
    );
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
}

#[test]
fn cli_project_archive_restores_dashboard_and_task_visibility_in_both_formats() {
    for format in ["markdown", "obsidian"] {
        let cli = Cli::new(format);
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
        cli.ok(&["project", "archive", &project]);
        assert_eq!(properties(&board)["status"], "ARCHIVED");
        assert_eq!(properties(note)["archived"], true);
        assert!(
            properties(note)["tags"]
                .as_array()
                .unwrap()
                .contains(&json!("archived"))
        );
        assert!(cli.ok(&["project", "list"]).as_array().unwrap().is_empty());
        if format == "markdown" {
            assert!(
                !fs::read_to_string(root.join("Dashboard.md"))
                    .unwrap()
                    .contains("[Demo](")
            );
        } else {
            let base: Value =
                serde_yaml::from_str(&fs::read_to_string(root.join("Dashboard.base")).unwrap())
                    .unwrap();
            assert!(
                base["filters"]["and"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("note.status == \"ACTIVE\""))
            );
        }
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
}

#[test]
fn cli_dependency_graph_and_task_notes_follow_cross_job_changes() {
    for format in ["markdown", "obsidian"] {
        let cli = Cli::new(format);
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
        cli.ok(&["job", "archive", &upstream]);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("Approved design (Job: Upstream) · DONE"));
        assert!(text.contains(&format!("{a} --> {b}")));
        assert_eq!(text.matches(&format!("{a}[\"")).count(), 1);
        assert!(text.contains(if format == "obsidian" {
            "Approved design.md"
        } else {
            "Approved%20design.md"
        }));
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
}

#[test]
fn cli_projects_every_status_to_mermaid_and_tasknotes_with_matching_colors() {
    let settings: Value = serde_json::from_str(include_str!(
        "../../../../plugins/agent-task-manager/obsidian/tasknotes-settings.json"
    ))
    .unwrap();
    for format in ["markdown", "obsidian"] {
        let cli = Cli::new(format);
        let job = cli.job("Every state");
        for setting in settings["customStatuses"].as_array().unwrap() {
            let status = setting["value"].as_str().unwrap();
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
            let color = setting["color"].as_str().unwrap();
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
}
