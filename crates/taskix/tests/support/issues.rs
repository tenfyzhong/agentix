use super::*;

#[test]
fn retired_intake_commands_and_job_flags_are_unavailable() {
    let cli = Cli::new();
    for args in [
        vec!["inbox", "list"],
        vec!["job", "create", "--title", "Old", "--inbox", "inbox_old"],
        vec!["job", "update", "job_old", "--inbox", "inbox_old"],
        vec![
            "job",
            "followup",
            "job_old",
            "--prompt",
            "Old",
            "--inbox",
            "inbox_old",
        ],
    ] {
        assert_eq!(
            cli.run(&args).status.code(),
            Some(2),
            "parser must reject {args:?}"
        );
    }
    let help = cli.run(&["--help"]);
    assert!(!String::from_utf8_lossy(&help.stdout).contains("inbox"));
}

#[test]
fn context_exposes_issue_and_milestone_without_inbox_compatibility() {
    let cli = Cli::new();
    let project = cli.ok(&["project", "register", "--name", "Demo"]);
    let project = project["id"].as_str().unwrap();
    let milestone = cli.ok(&[
        "milestone",
        "create",
        "--project",
        project,
        "--title",
        "MVP",
        "--goal",
        "Ship login",
    ]);
    let item = cli.ok(&[
        "issue",
        "create",
        "--project",
        project,
        "--title",
        "Login",
        "--requirement",
        "Allow login",
        "--acceptance",
        "Valid users sign in",
        "--milestone",
        milestone["id"].as_str().unwrap(),
    ]);
    let job = cli.ok(&[
        "job",
        "create",
        "--project",
        project,
        "--title",
        "Round",
        "--issue",
        item["id"].as_str().unwrap(),
    ]);
    let context = cli.ok(&["context", "--job", job["id"].as_str().unwrap()]);
    assert_eq!(context["issue_id"], item["id"]);
    assert_eq!(context["issue"]["requirement"], "Allow login");
    assert_eq!(context["milestone"]["goal"], "Ship login");
    assert_eq!(context["issue_candidates"][0]["id"], item["id"]);
    for field in ["inbox", "inbox_path", "inbox_todos", "inbox_cancellations"] {
        assert!(context.get(field).is_none(), "{field}");
    }
}

#[test]
fn job_issue_link_rejects_stale_requirement_guards() {
    let cli = Cli::new();
    let project = cli.ok(&["project", "register", "--name", "Demo"]);
    let item = cli.ok(&[
        "issue",
        "create",
        "--project",
        project["id"].as_str().unwrap(),
        "--title",
        "Login",
    ]);
    let job = cli.ok(&[
        "job",
        "create",
        "--project",
        project["id"].as_str().unwrap(),
        "--title",
        "Round",
    ]);
    let id = job["id"].as_str().unwrap();
    let issue = item["id"].as_str().unwrap();
    assert!(
        !cli.run(&[
            "job",
            "update",
            id,
            "--issue",
            issue,
            "--issue-revision",
            "0"
        ])
        .status
        .success()
    );
    assert!(cli.ok(&["job", "show", id])["issue_id"].is_null());
    let linked = cli.ok(&[
        "job",
        "update",
        id,
        "--issue",
        issue,
        "--issue-revision",
        &item["revision"].to_string(),
        "--issue-content-hash",
        item["content_hash"].as_str().unwrap(),
    ]);
    assert_eq!(linked["issue_id"], item["id"]);
}

#[test]
fn obsidian_sync_imports_one_delivery_document_without_repeating_updates() {
    let cli = Cli::new();
    let project = cli.ok(&["project", "register", "--name", "Demo"]);
    let item = cli.ok(&[
        "issue",
        "create",
        "--project",
        project["id"].as_str().unwrap(),
        "--title",
        "Login",
        "--requirement",
        "Original",
    ]);
    let id = item["id"].as_str().unwrap();
    let doc = cli
        .dir
        .path()
        .join("vault/Tasks \u{2603}")
        .join(item["document_path"].as_str().unwrap());
    std::fs::write(
        &doc,
        std::fs::read_to_string(&doc)
            .unwrap()
            .replace("Original", "Authored"),
    )
    .unwrap();
    cli.ok(&["obsidian", "sync", id]);
    let current = cli.ok(&["issue", "show", id])["issue"].clone();
    assert_eq!(current["requirement"], "Authored");
    cli.ok(&["obsidian", "sync", id]);
    assert_eq!(
        cli.ok(&["issue", "show", id])["issue"]["revision"],
        current["revision"]
    );
    assert!(
        !cli.run(&["obsidian", "sync", "issue_missing"])
            .status
            .success()
    );
}

#[test]
fn issue_cli_supports_optional_job_links_offline_acceptance_and_filtering() {
    let cli = Cli::new();
    let project = cli.ok(&["project", "register", "--name", "Demo"]);
    let project = project["id"].as_str().unwrap();
    let stage = cli.ok(&[
        "milestone",
        "create",
        "--project",
        project,
        "--title",
        "MVP",
        "--goal",
        "Ship login",
        "--due-date",
        "2026-10-20",
    ]);
    let stage = stage["id"].as_str().unwrap();
    let item = cli.ok(&[
        "issue",
        "create",
        "--project",
        project,
        "--title",
        "Login",
        "--requirement",
        "Allow login",
        "--acceptance",
        "Valid users sign in",
        "--milestone",
        stage,
    ]);
    let id = item["id"].as_str().unwrap();
    assert_eq!(item["status"], "TODO");
    let job = cli.ok(&[
        "job",
        "create",
        "--project",
        project,
        "--title",
        "First round",
        "--issue",
        id,
    ]);
    assert_eq!(job["issue_id"], id);
    assert_eq!(
        cli.ok(&["issue", "show", id])["issue"]["milestone_id"],
        stage
    );
    assert!(!cli.run(&["issue", "submit", id]).status.success());
    cli.ok(&["job", "cancel", job["id"].as_str().unwrap()]);
    cli.ok(&["issue", "submit", id]);
    assert!(
        !cli.run(&["issue", "approve", id, "--executor", "agent:codex"])
            .status
            .success()
    );
    assert_eq!(cli.ok(&["issue", "approve", id])["status"], "COMPLETED");
    let page = cli.ok(&[
        "issue",
        "list",
        "--project",
        project,
        "--status",
        "COMPLETED",
        "--milestone",
        stage,
        "--limit",
        "1",
    ]);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], id);
    assert_eq!(
        cli.ok(&["issue", "list", "--project", project, "--status", "ACTIVE"])["items"],
        json!([])
    );
    cli.ok(&[
        "job",
        "update",
        job["id"].as_str().unwrap(),
        "--unlink-issue",
    ]);
    assert!(cli.ok(&["job", "show", job["id"].as_str().unwrap()])["issue_id"].is_null());
    assert!(!cli.run(&["milestone", "delete", stage]).status.success());
    cli.ok(&["issue", "update", id, "--unlink-milestone"]);
    cli.ok(&["milestone", "delete", stage]);
}

#[test]
fn issue_cli_paginates_after_filtering_and_preserves_milestone_progress() {
    let cli = Cli::new();
    let project = cli.ok(&["project", "register", "--name", "Demo"]);
    let project = project["id"].as_str().unwrap();
    let stage = cli.ok(&[
        "milestone",
        "create",
        "--project",
        project,
        "--title",
        "Release",
    ]);
    let stage = stage["id"].as_str().unwrap();
    for n in 0..9 {
        let item = cli.ok(&[
            "issue",
            "create",
            "--project",
            project,
            "--title",
            &format!("Item {n}"),
            "--milestone",
            stage,
        ]);
        if n < 3 {
            cli.ok(&["issue", "cancel", item["id"].as_str().unwrap()]);
        }
    }
    let page = cli.ok(&[
        "issue",
        "list",
        "--project",
        project,
        "--status",
        "TODO",
        "--limit",
        "4",
    ]);
    assert_eq!(page["total"], 6);
    assert_eq!(page["items"].as_array().unwrap().len(), 4);
    let after = page["next_cursor"].as_str().unwrap();
    let next = cli.ok(&[
        "issue",
        "list",
        "--project",
        project,
        "--status",
        "TODO",
        "--limit",
        "4",
        "--after",
        after,
    ]);
    assert_eq!(next["items"].as_array().unwrap().len(), 2);
    assert_eq!(next["complete"], true);
    let progress = cli.ok(&["milestone", "show", stage]);
    assert_eq!(progress["progress"]["total"], 9);
    assert_eq!(progress["progress"]["completed"], 0);
    assert_eq!(progress["progress"]["cancelled"], 3);
}
