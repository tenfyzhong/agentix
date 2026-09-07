use super::*;

#[test]
fn job_review_cli_routes_decisions_and_filters_pending_jobs() {
    let cli = Cli::new("markdown");
    let job = cli.job("Review lifecycle");
    let id = cli.task(&job, "Implement");
    let claim = cli.claim(&id, "review");
    cli.owned(
        &["plan", "create", &id, "--body", "# Implement and test"],
        &claim,
    );
    cli.owned(&["task", "start", &id], &claim);
    cli.owned(&["task", "done", &id], &claim);
    assert_eq!(cli.ok(&["job", "list", "--pending-review"])[0]["id"], job);
    assert!(
        cli.ok(&["job", "list", "--completed"])
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        cli.ok(&["job", "reject", &job, "--reason", "Needs revision"])["status"],
        "ACTIVE"
    );
    assert_eq!(cli.ok(&["job", "submit", &job])["status"], "PENDING_REVIEW");
    assert_eq!(cli.ok(&["job", "approve", &job])["status"], "COMPLETED");
    assert!(
        cli.ok(&["job", "list", "--pending-review"])
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn finish(cli: &Cli, job: &str, session: &str) -> String {
    let task = cli.task(job, "Work");
    let claim = cli.claim(&task, session);
    cli.owned(
        &["plan", "create", &task, "--body", "Complete work"],
        &claim,
    );
    cli.owned(&["task", "start", &task], &claim);
    cli.owned(&["task", "done", &task], &claim);
    task
}

#[test]
fn followup_cli_reuses_job_and_adds_dependencies() {
    let cli = Cli::new("markdown");
    let job = cli.job("Original");
    let previous = finish(&cli, &job, "followup");
    assert_eq!(
        cli.ok(&["job", "followup", &job, "--prompt", "补充\n保留原文"])["status"],
        "ACTIVE"
    );
    let task = cli.task(&job, "Supplement");
    assert_eq!(
        cli.ok(&["task", "show", &task])["dependencies"],
        json!([previous])
    );
}

#[test]
fn review_policy_cli_completes_simple_jobs() {
    let cli = Cli::new("markdown");
    let job = cli.job("Setup");
    let project = cli.ok(&["job", "show", &job])["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let simple = cli.ok(&[
        "job",
        "create",
        "--project",
        &project,
        "--title",
        "Investigate",
        "--review-policy",
        "none",
    ]);
    let simple = simple["id"].as_str().unwrap();
    finish(&cli, simple, "simple");
    assert_eq!(cli.ok(&["job", "show", simple])["status"], "COMPLETED");
    assert_eq!(
        cli.ok(&["job", "update", &job, "--review-policy", "required"])["review_policy"],
        "required"
    );
}

#[test]
fn context_only_offers_latest_session_job_in_current_project() {
    let cli = Cli::new("markdown");
    let job = cli.job("Previous");
    let task = finish(&cli, &job, "context");
    let context = cli.ok(&["context", "--session", "context"]);
    assert_eq!(context["previous_job"]["id"], job);
    assert_eq!(context["previous_job"]["task_ids"], json!([task]));
    assert!(cli.ok(&["context", "--session", "other"])["previous_job"].is_null());
    assert!(cli.ok(&["context", "--session", "context", "--job", &job])["previous_job"].is_null());
    let other_root = cli.dir.path().join("other");
    std::fs::create_dir(&other_root).unwrap();
    let other = cli.ok(&[
        "project",
        "register",
        "--name",
        "Other",
        "--root",
        other_root.to_str().unwrap(),
    ]);
    assert!(
        cli.ok(&[
            "context",
            "--session",
            "context",
            "--project",
            other["id"].as_str().unwrap()
        ])["previous_job"]
            .is_null()
    );
    let latest = cli.job("Latest");
    finish(&cli, &latest, "context");
    cli.ok(&["job", "approve", &latest]);
    assert!(cli.ok(&["context", "--session", "context"])["previous_job"].is_null());
}

#[test]
fn context_followup_activity_supersedes_older_session_job() {
    let cli = Cli::new("markdown");
    let original = cli.job("Original");
    finish(&cli, &original, "first-session");
    let older = cli.job("Other work");
    finish(&cli, &older, "new-session");
    cli.ok(&[
        "job",
        "followup",
        &original,
        "--prompt",
        "More work",
        "--session",
        "new-session",
    ]);
    assert!(cli.ok(&["context", "--session", "new-session"])["previous_job"].is_null());
    cli.ok(&["job", "submit", &original]);
    assert_eq!(
        cli.ok(&["context", "--session", "new-session"])["previous_job"]["id"],
        original
    );
}
