use super::*;

#[tokio::test]
async fn job_list_filters_before_reading_excluded_bodies() {
    use sqlx::Connection;
    let cli = Cli::new();
    let target = cli.job("Selected");
    let other = cli.job("Excluded");
    let target_record = cli.ok(&["job", "show", &target]);
    let other_record = cli.ok(&["job", "show", &other]);
    let mut db = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(cli.dir.path().join("state.sqlite3")),
    )
    .await
    .unwrap();
    let january = 1_767_225_600_i64;
    let february = 1_769_904_000_i64;
    let cases: Vec<(&[&str], Value, Value)> = vec![
        (
            &["--active"],
            json!({"status":"ACTIVE"}),
            json!({"status":"COMPLETED"}),
        ),
        (
            &["--active"],
            json!({"status":"ACTIVE"}),
            json!({"status":"ACTIVE","archived_at":january}),
        ),
        (
            &["--pending-review"],
            json!({"status":"PENDING_REVIEW"}),
            json!({"status":"PENDING_REVIEW","archived_at":january}),
        ),
        (
            &["--completed"],
            json!({"status":"COMPLETED","archived_at":january}),
            json!({"status":"ACTIVE"}),
        ),
        (&["--archived"], json!({"archived_at":january}), json!({})),
        (
            &["--created-from", "2026-01-01"],
            json!({}),
            json!({"created_at":january-1}),
        ),
        (
            &["--created-to", "2026-01-01"],
            json!({"created_at":january+86399}),
            json!({"created_at":january+86400}),
        ),
        (
            &["--period", "2026-01"],
            json!({"archived_at":january}),
            json!({"archived_at":february}),
        ),
        (
            &["--period", "2026-01"],
            json!({"archived_at":february-1}),
            json!({"archived_at":january-1}),
        ),
    ];
    for (flags, selected, excluded) in cases {
        for (base, patch) in [(&target_record, selected), (&other_record, excluded)] {
            let mut record = base.clone();
            record["created_at"] = json!(january);
            record["archived_at"] = Value::Null;
            record
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            if record["id"] == other {
                record["title"] = json!({"unreadable":"excluded body"});
            }
            sqlx::query("UPDATE jobs SET data=? WHERE id=?")
                .bind(record.to_string())
                .bind(record["id"].as_str().unwrap())
                .execute(&mut db)
                .await
                .unwrap();
        }
        let mut args = vec!["job", "list"];
        args.extend_from_slice(flags);
        let result = cli.ok(&args);
        assert_eq!(result.as_array().unwrap().len(), 1, "{flags:?}");
        assert_eq!(result[0]["id"], target, "{flags:?}");
    }
}

#[test]
fn job_review_cli_routes_decisions_and_filters_pending_jobs() {
    let cli = Cli::new();
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
    let cli = Cli::new();
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
    let cli = Cli::new();
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
    let cli = Cli::new();
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
    let cli = Cli::new();
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
