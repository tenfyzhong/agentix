use super::*;

fn project(cli: &Cli) -> String {
    cli.ok(&[
        "project",
        "register",
        "--name",
        "Demo",
        "--root",
        cli.dir.path().to_str().unwrap(),
    ])["id"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn inbox_cli_preserves_content_claims_releases_and_cancels() {
    let cli = Cli::new("markdown");
    let project = project(&cli);
    let body = "Request title\n\nDescription with  two spaces.\n- [ ] Acceptance";
    let entry = cli.ok(&["inbox", "add", "--project", &project, "--content", body]);
    assert_eq!(entry["content"], body);
    let list = cli.ok(&["inbox", "list", "--project", &project]);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let claim = cli.ok(&[
        "inbox",
        "claim-next",
        "--project",
        &project,
        "--session",
        "s1",
        "--executor",
        "agent:codex",
    ]);
    assert_eq!(claim["claimed"], true);
    let id = entry["id"].as_str().unwrap();
    let context = cli.ok(&["context", "--session", "s1"]);
    assert_eq!(context["project_id"], project);
    assert_eq!(context["inbox"]["id"], id);
    assert_eq!(context["job_id"], claim["job"]["id"]);
    assert!(
        context["inbox_path"]
            .as_str()
            .unwrap()
            .ends_with("Inbox.md")
    );
    cli.ok(&[
        "inbox",
        "release",
        id,
        "--session",
        "s1",
        "--lease-token",
        claim["entry"]["lease"]["token"].as_str().unwrap(),
    ]);
    let second = cli.ok(&[
        "inbox",
        "claim-next",
        "--project",
        &project,
        "--session",
        "s2",
        "--executor",
        "agent:codex",
    ]);
    assert_eq!(second["job"]["id"], claim["job"]["id"]);
    cli.ok(&["inbox", "cancel", id]);
    assert_eq!(
        cli.ok(&["inbox", "list", "--project", &project])[0]["status"],
        "CANCELLED"
    );
}

#[test]
fn inbox_cancellation_is_reported_to_a_different_task_executor() {
    let cli = Cli::new("markdown");
    let project = project(&cli);
    let entry = cli.ok(&[
        "inbox",
        "add",
        "--project",
        &project,
        "--content",
        "Delegated work",
    ]);
    let claim = cli.ok(&[
        "inbox",
        "claim-next",
        "--project",
        &project,
        "--session",
        "coordinator",
        "--executor",
        "agent:codex",
    ]);
    let task = cli.ok(&[
        "task",
        "add",
        "--job",
        claim["job"]["id"].as_str().unwrap(),
        "--title",
        "Implementation",
    ]);
    cli.ok(&[
        "task",
        "claim",
        task["id"].as_str().unwrap(),
        "--session",
        "worker",
        "--executor",
        "agent:codex",
    ]);
    cli.ok(&["inbox", "cancel", entry["id"].as_str().unwrap()]);
    for command in [
        vec!["context", "--session", "worker"],
        vec!["hook", "heartbeat", "--session", "worker"],
    ] {
        let result = cli.ok(&command);
        assert_eq!(result["inbox_cancellations"][0]["id"], entry["id"]);
    }
}

#[test]
fn inbox_cli_processes_racing_to_claim_have_one_winner() {
    let cli = Cli::new("markdown");
    let project = project(&cli);
    for title in ["One", "Two"] {
        cli.ok(&["inbox", "add", "--project", &project, "--content", title]);
    }
    let mut children = Vec::new();
    for i in 0..8 {
        children.push(
            cli.command(&[
                "inbox",
                "claim-next",
                "--project",
                &project,
                "--session",
                &format!("worker-{i}"),
                "--executor",
                "agent:codex",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
        );
    }
    let mut winners = 0;
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        winners += usize::from(response["result"]["claimed"] == true);
    }
    assert_eq!(winners, 1);
    assert_eq!(
        cli.ok(&["job", "list", "--project", &project])
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn inbox_stop_requires_prior_session_work_and_does_not_loop() {
    let cli = Cli::new("markdown");
    let project = project(&cli);
    cli.ok(&[
        "inbox",
        "add",
        "--project",
        &project,
        "--content",
        "Next requirement",
    ]);
    let stop = [
        "hook",
        "stop",
        "--project",
        &project,
        "--session",
        "worker",
        "--executor",
        "agent:codex",
    ];
    assert_eq!(cli.ok(&stop)["claimed"], false);
    let job = cli.ok(&[
        "job",
        "create",
        "--project",
        &project,
        "--title",
        "Previous",
        "--session",
        "worker",
        "--executor",
        "agent:codex",
    ]);
    cli.ok(&["job", "cancel", job["id"].as_str().unwrap()]);
    assert_eq!(cli.ok(&stop)["claimed"], true);
    assert_eq!(cli.ok(&stop)["claimed"], false);
    let job_id = cli.ok(&["context", "--session", "worker"])["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    cli.ok(&[
        "inbox",
        "cancel",
        cli.ok(&["inbox", "list", "--project", &project])[0]["id"]
            .as_str()
            .unwrap(),
    ]);
    let context = cli.ok(&["context", "--session", "worker"]);
    assert_eq!(context["project_id"], project);
    assert!(
        context["inbox_cancellations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["job_id"] == job_id)
    );
    let heartbeat = cli.ok(&["hook", "heartbeat", "--session", "worker"]);
    assert!(
        heartbeat["inbox_cancellations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["job_id"] == job_id)
    );
}

#[test]
fn inbox_context_prefers_current_directory_over_ambiguous_session_history() {
    let cli = Cli::new("markdown");
    let first = project(&cli);
    let other = cli.dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let second = cli.ok(&[
        "project",
        "register",
        "--name",
        "Other",
        "--root",
        other.to_str().unwrap(),
    ])["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for p in [&first, &second] {
        cli.ok(&[
            "job",
            "create",
            "--project",
            p,
            "--title",
            "Previous work",
            "--session",
            "worker",
            "--executor",
            "agent:codex",
        ]);
    }
    let context = cli.ok(&["context", "--session", "worker"]);
    assert_eq!(context["project_id"], first);
}
