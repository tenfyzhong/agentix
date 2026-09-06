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
