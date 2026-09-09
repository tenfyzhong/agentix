use super::*;

#[tokio::test]
async fn inbox_review_marker_migration_preserves_state_and_authored_details() {
    let mut f = fixture().await;
    add(&f, "Review\n\nKeep literal - [p] examples.").await;
    let claimed = claim(&f, "worker").await;
    f.job = claimed["job"]["id"].as_str().unwrap().into();
    let task = f.task("Delivery").await;
    let owned = f.start(&task, "worker").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&owned))
        .await
        .unwrap();
    let before = f.service.store().snapshot().await.unwrap().inboxes;
    for marker in ["p", "r"] {
        let doc = std::fs::read_to_string(path(&f)).unwrap();
        let doc = doc
            .replace("- [p] Review", &format!("- [{marker}] Review"))
            .replace("- [r] Review", &format!("- [{marker}] Review"));
        std::fs::write(path(&f), doc).unwrap();
        f.service.sync().await.unwrap();
        let doc = std::fs::read_to_string(path(&f)).unwrap();
        assert!(doc.contains("- [r] Review"));
        assert!(doc.contains("Keep literal - [p] examples."));
        assert_eq!(f.service.store().snapshot().await.unwrap().inboxes, before);
    }
}

const END: &str = "<!-- taskcli:inbox:end -->";

#[tokio::test]
async fn inbox_pending_review_allows_next_claim_and_rejection_blocks_it() {
    let mut f = fixture().await;
    add(&f, "First").await;
    add(&f, "Second").await;
    add(&f, "Third").await;
    let first = claim(&f, "one").await;
    f.job = first["job"]["id"].as_str().unwrap().into();
    let task = f.task("Deliver first").await;
    let owned = f.start(&task, "one").await;
    assert_eq!(claim(&f, "two").await["reason"], "active_jobs");
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&owned))
        .await
        .unwrap();
    let review = f
        .service
        .store()
        .snapshot()
        .await
        .unwrap()
        .jobs
        .iter()
        .find(|job| job.id == f.job)
        .unwrap()
        .clone();
    let next = claim(&f, "two").await;
    assert_eq!(next["claimed"], true, "{next}");
    assert_ne!(next["job"]["id"], f.job);
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(
        *state.jobs.iter().find(|job| job.id == f.job).unwrap(),
        review
    );
    assert_eq!(entries(&f).await[0]["status"], "PENDING_REVIEW");
    assert!(entries(&f).await[0]["lease"].is_null());
    assert_eq!(claim(&f, "three").await["reason"], "active_jobs");
    f.service
        .execute(
            json!({"command":"inbox.cancel","inbox":next["entry"]["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"job.reject","job":f.job,"reason":"Needs repair"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let recovered = claim(&f, "repair").await;
    assert_eq!(recovered["job"]["id"], f.job);
    assert_eq!(claim(&f, "three").await["reason"], "active_jobs");
}

#[tokio::test]
async fn inbox_aligned_preflight_does_not_publish_an_old_checkbox_before_validation() {
    let f = fixture().await;
    let entry = add(&f, "No stale echo").await;
    let source = std::fs::read_to_string(path(&f))
        .unwrap()
        .replace("- [ ] No stale echo", "- [x] No stale echo");
    std::fs::write(path(&f), &source).unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"inbox.set-status","inbox":entry["id"],"status":"COMPLETED"}),
            WriteOptions {
                expected_revision: Some(999),
                ..WriteOptions::default()
            },
        )
        .await;
    assert!(result.unwrap_err().to_string().contains("revision"));
    assert_eq!(
        std::fs::read_to_string(path(&f)).unwrap(),
        source,
        "preflight must not echo the old checkbox while the plugin has this intent in flight"
    );
}

#[tokio::test]
async fn inbox_aligned_sync_repairs_reopened_cancellation_after_a_projection_interruption() {
    let f = fixture().await;
    let entry = add(&f, "Recover publication").await;
    let id = entry["id"].as_str().unwrap();
    set_status(&f, id, "CANCELLED").await.unwrap();
    // Simulate a committed write whose process exits before publishing Markdown.
    f.service
        .store()
        .execute(
            json!({"command":"inbox.set-status","inbox":id,"status":"TODO"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("- [-] Recover publication")
    );
    f.service.sync().await.unwrap();
    assert_eq!(entries(&f).await[0]["status"], "TODO");
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("- [ ] Recover publication")
    );
}

#[tokio::test]
async fn inbox_aligned_states_follow_job_review_and_use_distinct_checkboxes() {
    let mut f = fixture().await;
    let item = add(&f, "Five states").await;
    let id = item["id"].as_str().unwrap();
    let active = set_status(&f, id, "ACTIVE").await.unwrap().result;
    assert_eq!(active["status"], "ACTIVE");
    assert!(
        active["lease"].is_null(),
        "manual activation must not impersonate an agent"
    );
    assert!(active["job_id"].is_null());
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("- [/] Five states")
    );
    let claimed = claim(&f, "worker").await;
    assert_eq!(claimed["claimed"], true);
    f.job = claimed["job"]["id"].as_str().unwrap().into();
    assert!(set_status(&f, id, "PENDING_REVIEW").await.is_err());
    let task = f.task("Delivery").await;
    let owned = f.start(&task, "worker").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&owned))
        .await
        .unwrap();
    assert_eq!(entries(&f).await[0]["status"], "PENDING_REVIEW");
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("- [r] Five states")
    );
    assert_eq!(
        set_status(&f, id, "ACTIVE").await.unwrap().result["status"],
        "ACTIVE"
    );
    let snapshot = f.service.store().snapshot().await.unwrap();
    assert_eq!(snapshot.tasks[0].status.to_string(), "DONE");
    assert_eq!(
        snapshot
            .jobs
            .iter()
            .find(|j| j.id == f.job)
            .unwrap()
            .status
            .to_string(),
        "ACTIVE"
    );
    let events = f
        .service
        .store()
        .events(Some(&f.job), 0, 100)
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "job.rejected")
    );
    assert_eq!(
        set_status(&f, id, "PENDING_REVIEW").await.unwrap().result["status"],
        "PENDING_REVIEW"
    );
    assert_eq!(
        set_status(&f, id, "COMPLETED").await.unwrap().result["status"],
        "COMPLETED"
    );
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("- [x] Five states")
    );
    set_status(&f, id, "ACTIVE").await.unwrap();
    set_status(&f, id, "CANCELLED").await.unwrap();
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("- [-] Five states")
    );
    set_status(&f, id, "TODO").await.unwrap();
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("- [ ] Five states")
    );
}

#[tokio::test]
async fn inbox_aligned_schema_migrates_legacy_states_and_pending_review_idempotently() {
    let mut f = fixture().await;
    let item = add(&f, "Pending").await;
    let claimed = claim(&f, "worker").await;
    f.job = claimed["job"]["id"].as_str().unwrap().into();
    let task = f.task("Done work").await;
    let owned = f.start(&task, "worker").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&owned))
        .await
        .unwrap();
    let manual = add(&f, "Finished manually").await;
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(&f.service.config().storage.path),
        )
        .await
        .unwrap();
    for (id, status) in [(&item["id"], "IN_PROGRESS"), (&manual["id"], "DONE")] {
        sqlx::query("UPDATE inbox_entries SET data=json_set(data,'$.status',?) WHERE id=?")
            .bind(status)
            .bind(id.as_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("PRAGMA user_version = 9")
        .execute(&pool)
        .await
        .unwrap();
    let migrated = Store::open(&f.service.config().storage.path).await.unwrap();
    let state = migrated.snapshot().await.unwrap();
    let rows = serde_json::to_value(&state.inboxes).unwrap();
    assert_eq!(rows[0]["status"], "PENDING_REVIEW");
    assert_eq!(rows[1]["status"], "COMPLETED");
    assert_eq!(rows[0]["job_id"], f.job);
    let again = Store::open(&f.service.config().storage.path).await.unwrap();
    assert_eq!(again.snapshot().await.unwrap().inboxes, state.inboxes);
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 12);
}

#[tokio::test]
async fn inbox_aligned_import_accepts_slash_and_review_without_replaying_status_edits() {
    let f = fixture().await;
    let initial = std::fs::read_to_string(path(&f)).unwrap();
    std::fs::write(
        path(&f),
        initial.replace(
            END,
            &format!("- [/] Example active\n- [r] Example review\n{END}"),
        ),
    )
    .unwrap();
    f.service.sync().await.unwrap();
    let rows = entries(&f).await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["status"] == "TODO"));
}

async fn set_status(f: &Fixture, id: &str, status: &str) -> anyhow::Result<agentix_task::Outcome> {
    f.service
        .execute(
            json!({"command":"inbox.set-status","inbox":id,"status":status}),
            WriteOptions::default(),
        )
        .await
}

#[tokio::test]
async fn inbox_manual_status_changes_do_not_create_jobs() {
    let f = fixture().await;
    let item = add(&f, "Manual states").await;
    let id = item["id"].as_str().unwrap();
    let jobs = f.service.store().snapshot().await.unwrap().jobs;
    for (status, marker) in [
        ("ACTIVE", '/'),
        ("PENDING_REVIEW", 'r'),
        ("COMPLETED", 'x'),
        ("ACTIVE", '/'),
        ("CANCELLED", '-'),
        ("ACTIVE", '/'),
        ("TODO", ' '),
        ("IN_PROGRESS", '/'),
    ] {
        let entry = set_status(&f, id, status).await.unwrap().result;
        let expected = if status == "IN_PROGRESS" {
            "ACTIVE"
        } else {
            status
        };
        assert_eq!(entry["status"], expected);
        assert!(entry["job_id"].is_null(), "manual {status} created a Job");
        assert!(entry["lease"].is_null());
        f.service.sync().await.unwrap();
        let state = f.service.store().snapshot().await.unwrap();
        assert_eq!(state.jobs, jobs);
        assert_eq!(state.inboxes[0].status.to_string(), expected);
        let doc = std::fs::read_to_string(path(&f)).unwrap();
        assert!(doc.contains(&format!("- [{marker}] Manual states")));
    }
    let claimed = claim(&f, "worker").await;
    assert_eq!(claimed["claimed"], true);
    assert_eq!(claimed["entry"]["id"], id);
    assert_eq!(claimed["entry"]["job_id"], claimed["job"]["id"]);
    assert_eq!(
        f.service.store().snapshot().await.unwrap().jobs.len(),
        jobs.len() + 1
    );
}

#[tokio::test]
async fn inbox_status_completes_unlinked_items_and_reopens_without_creating_jobs() {
    let f = fixture().await;
    let item = add(&f, "Manual work").await;
    let id = item["id"].as_str().unwrap();
    assert_eq!(
        set_status(&f, id, "DONE").await.unwrap().result["status"],
        "COMPLETED"
    );
    assert_eq!(
        set_status(&f, id, "TODO").await.unwrap().result["status"],
        "TODO"
    );
    assert_eq!(
        set_status(&f, id, "CANCELLED").await.unwrap().result["status"],
        "CANCELLED"
    );
    assert_eq!(
        set_status(&f, id, "TODO").await.unwrap().result["status"],
        "TODO"
    );
    assert_eq!(f.service.store().snapshot().await.unwrap().jobs.len(), 1);
    assert!(set_status(&f, id, "UNKNOWN").await.is_err());
    let snapshot = f.service.obsidian_snapshot().await.unwrap();
    let note = snapshot["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .unwrap();
    assert_eq!(note["kind"], "inbox");
    assert!(
        note["path"]
            .as_str()
            .unwrap()
            .ends_with("Projects/demo/Inbox.md")
    );
    assert_eq!(note["revision"], entries(&f).await[0]["revision"]);
    assert!(note.get("lease").is_none());
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("revision=")
    );
}

#[tokio::test]
async fn inbox_status_requires_review_and_reopens_the_same_job_preserving_tasks() {
    let mut f = fixture().await;
    let entry = add(&f, "Deliver").await;
    let id = entry["id"].as_str().unwrap();
    let claimed = claim(&f, "one").await;
    f.job = claimed["job"]["id"].as_str().unwrap().into();
    assert!(
        set_status(&f, id, "DONE")
            .await
            .unwrap_err()
            .to_string()
            .contains("PENDING_REVIEW")
    );
    assert!(set_status(&f, id, "TODO").await.is_err());
    let task = f.task("Implementation").await;
    let owned = f.start(&task, "one").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&owned))
        .await
        .unwrap();
    assert_eq!(
        set_status(&f, id, "DONE").await.unwrap().result["status"],
        "COMPLETED"
    );
    let before = f.service.store().snapshot().await.unwrap();
    assert_eq!(
        set_status(&f, id, "TODO").await.unwrap().result["status"],
        "TODO"
    );
    f.service.sync().await.unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.tasks, before.tasks);
    let job = state.jobs.iter().find(|j| j.id == f.job).unwrap();
    assert_eq!(job.status.to_string(), "ACTIVE");
    assert!(job.completed_at.is_none());
    assert_eq!(claim(&f, "two").await["job"]["id"], f.job);
}

#[tokio::test]
async fn inbox_status_fences_checkbox_cancellation_and_replays_once() {
    let f = fixture().await;
    let entry = add(&f, "Cancel me").await;
    let row = entries(&f).await.remove(0);
    let id = entry["id"].as_str().unwrap();
    let source = std::fs::read_to_string(path(&f)).unwrap();
    std::fs::write(
        path(&f),
        source.replace("- [ ] Cancel me", "- [-] Cancel me"),
    )
    .unwrap();
    let options = WriteOptions {
        expected_revision: row["revision"].as_i64(),
        idempotency_key: Some("cancel-checkbox".into()),
        ..WriteOptions::default()
    };
    let request = json!({"command":"inbox.set-status","inbox":id,"status":"CANCELLED"});
    let first = f
        .service
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    assert_eq!(first.result["status"], "CANCELLED");
    assert_eq!(
        f.service
            .execute(request, options.clone())
            .await
            .unwrap()
            .sequence,
        first.sequence
    );
    assert!(
        f.service
            .execute(
                json!({"command":"inbox.set-status","inbox":id,"status":"TODO"}),
                WriteOptions {
                    idempotency_key: None,
                    ..options
                }
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("revision")
    );
}

#[tokio::test]
async fn inbox_status_does_not_revive_withdrawn_or_archived_work() {
    let f = fixture().await;
    let entry = add(&f, "Withdraw").await;
    let id = entry["id"].as_str().unwrap();
    let source = std::fs::read_to_string(path(&f)).unwrap();
    let source = source
        .lines()
        .filter(|line| !line.contains(id))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path(&f), source).unwrap();
    assert!(set_status(&f, id, "TODO").await.is_err());
    let entry = add(&f, "Archive").await;
    let id = entry["id"].as_str().unwrap();
    set_status(&f, id, "DONE").await.unwrap();
    f.service
        .execute(
            json!({"command":"project.archive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(set_status(&f, id, "TODO").await.is_err());
}

#[tokio::test]
async fn inbox_rejects_reserved_control_markers_before_committing_a_submission() {
    let f = fixture().await;
    let before = entries(&f).await;
    for content in [
        "Request\n<!-- taskcli:inbox:end -->",
        "Request\n<!-- taskcli:entry-state --> TODO",
    ] {
        let result = f
            .service
            .execute(
                json!({"command":"inbox.add","project":f.project,"content":content}),
                WriteOptions::default(),
            )
            .await;
        assert!(
            result.is_err(),
            "reserved content must not commit: {content}"
        );
        assert_eq!(entries(&f).await, before);
    }
    add(&f, "Valid request").await;
    assert_eq!(entries(&f).await.len(), 1);
}

#[tokio::test]
async fn inbox_session_project_does_not_guess_an_outer_project_for_a_nested_repository() {
    let f = fixture().await;
    let state = f.service.store().snapshot().await.unwrap();
    let root = std::path::Path::new(&state.projects[0].root);
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&nested)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        f.service
            .project_for_session(Some(&nested), None)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.service
            .project_for_session(Some(root), None)
            .await
            .unwrap()
            .unwrap()
            .id,
        f.project
    );
}

async fn fixture() -> Fixture {
    let f = Fixture::new().await;
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f
}

fn path(f: &Fixture) -> std::path::PathBuf {
    f.service
        .config()
        .output_dir()
        .join("Projects/demo/Inbox.md")
}

async fn entries(f: &Fixture) -> Vec<Value> {
    serde_json::to_value(f.service.store().snapshot().await.unwrap()).unwrap()["inboxes"]
        .as_array()
        .unwrap()
        .clone()
}

async fn add(f: &Fixture, content: &str) -> Value {
    f.service
        .execute(
            json!({"command":"inbox.add","project":f.project,"content":content}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result
}

fn identity(session: &str) -> WriteOptions {
    WriteOptions {
        actor_ref: format!("agent:{session}"),
        session_ref: Some(session.into()),
        ..WriteOptions::default()
    }
}

async fn claim(f: &Fixture, session: &str) -> Value {
    f.service
        .execute(
            json!({"command":"inbox.claim-next","project":f.project}),
            identity(session),
        )
        .await
        .unwrap()
        .result
}

#[tokio::test]
async fn inbox_metadata_stays_on_the_header_with_status_in_a_comment() {
    let f = fixture().await;
    let content = "Request\nDetails with **Markdown**.\n- [ ] Acceptance";
    let entry = add(&f, content).await;
    let id = entry["id"].as_str().unwrap();
    let source = std::fs::read_to_string(path(&f)).unwrap();
    let revision = entries(&f).await[0]["revision"].as_i64().unwrap();
    assert!(source.contains(&format!(
        "- [ ] Request <!-- taskcli:entry:{id} --> <!-- taskcli:entry-state TODO revision={revision} -->\n  Details with **Markdown**.\n  - [ ] Acceptance\n"
    )));
    assert!(!source.contains("\n  <!-- taskcli:entry-state"));
    f.service.sync().await.unwrap();
    assert_eq!(std::fs::read_to_string(path(&f)).unwrap(), source);
    assert_eq!(entries(&f).await[0]["content"], content);

    let claimed = claim(&f, "one").await;
    assert_eq!(claimed["claimed"], true);
    let source = std::fs::read_to_string(path(&f)).unwrap();
    let header = source
        .lines()
        .find(|line| line.starts_with("- [/] Request"))
        .unwrap();
    let revision = entries(&f).await[0]["revision"].as_i64().unwrap();
    assert!(header.contains(&format!(
        "<!-- taskcli:entry-state ACTIVE revision={revision} --> · "
    )));
    assert!(header.contains("[["));
    assert!(header.ends_with(" · agent:one"));
    assert!(!source.contains("\n  <!-- taskcli:entry-state"));
    f.service.sync().await.unwrap();
    assert_eq!(std::fs::read_to_string(path(&f)).unwrap(), source);
    assert_eq!(entries(&f).await[0]["content"], content);

    f.service
        .execute(
            json!({"command":"inbox.cancel","inbox":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let source = std::fs::read_to_string(path(&f)).unwrap();
    let revision = entries(&f).await[0]["revision"].as_i64().unwrap();
    assert!(source.contains(&format!(
        "- [-] Request <!-- taskcli:entry:{id} --> <!-- taskcli:entry-state CANCELLED revision={revision} --> · "
    )));
    assert!(!source.contains("agent:one"));
}

#[tokio::test]
async fn inbox_legacy_receipt_migrates_without_changing_identity_or_content() {
    let f = fixture().await;
    let initial = std::fs::read_to_string(path(&f)).unwrap();
    let id = "inbox_01a07760d6a673f2a863e0f105eb9783";
    let legacy = format!(
        "- [ ] Request <!-- taskcli:entry:{id} -->\n  Details.\n  <!-- taskcli:entry-state --> TODO\n\n"
    );
    std::fs::write(path(&f), initial.replace(END, &format!("{legacy}{END}"))).unwrap();
    f.service.sync().await.unwrap();
    let rows = entries(&f).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id);
    assert_eq!(rows[0]["content"], "Request\nDetails.");
    assert_eq!(rows[0]["status"], "TODO");
    let source = std::fs::read_to_string(path(&f)).unwrap();
    let revision = rows[0]["revision"].as_i64().unwrap();
    assert!(source.contains(&format!(
        "- [ ] Request <!-- taskcli:entry:{id} --> <!-- taskcli:entry-state TODO revision={revision} -->\n  Details.\n"
    )));
    f.service.sync().await.unwrap();
    assert_eq!(entries(&f).await, rows);
    assert_eq!(std::fs::read_to_string(path(&f)).unwrap(), source);
}

#[tokio::test]
async fn inbox_import_preserves_markdown_and_ignores_nested_and_fenced_checklists() {
    let f = fixture().await;
    let initial = std::fs::read_to_string(path(&f)).unwrap();
    let authored = "- [ ] First\n  Details with **Markdown**.\n  - [ ] Nested acceptance\n\n```md\n- [ ] Example only\n```\n\n- [ ] First\n";
    std::fs::write(
        path(&f),
        initial.replace(END, &format!("{authored}\n{END}")),
    )
    .unwrap();
    f.service.sync().await.unwrap();
    let rows = entries(&f).await;
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0]["id"], rows[1]["id"]);
    assert!(
        rows[0]["content"]
            .as_str()
            .unwrap()
            .contains("- [ ] Nested acceptance")
    );
    let rendered = std::fs::read_to_string(path(&f)).unwrap();
    assert!(rendered.contains("```md\n- [ ] Example only\n```"));
    f.service.sync().await.unwrap();
    assert_eq!(entries(&f).await, rows);
    assert_eq!(std::fs::read_to_string(path(&f)).unwrap(), rendered);
}

#[tokio::test]
async fn inbox_claim_is_exclusive_and_waits_for_all_project_jobs() {
    let f = Fixture::new().await;
    add(&f, "First").await;
    add(&f, "Second").await;
    assert_eq!(claim(&f, "one").await["reason"], "active_jobs");
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let (a, b) = tokio::join!(claim(&f, "one"), claim(&f, "two"));
    assert_ne!(a["claimed"], b["claimed"]);
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.jobs.len(), 2);
    assert_eq!(
        entries(&f)
            .await
            .iter()
            .filter(|r| r["status"] == "ACTIVE")
            .count(),
        1
    );
}

#[tokio::test]
async fn inbox_cancellation_revokes_task_ownership_and_deletion_preserves_history() {
    let f = fixture().await;
    let entry = add(&f, "Deliver feature\nKeep this description.").await;
    let claimed = claim(&f, "one").await;
    let job = claimed["job"]["id"].as_str().unwrap();
    let task = f
        .service
        .execute(
            json!({"command":"task.add","job":job,"title":"Implementation"}),
            identity("one"),
        )
        .await
        .unwrap()
        .result;
    let task_id = task["id"].as_str().unwrap();
    let owned = f.claim(task_id, "one").await;
    let text = std::fs::read_to_string(path(&f))
        .unwrap()
        .replace("- [/] Deliver", "- [-] Deliver");
    std::fs::write(path(&f), text).unwrap();
    f.service.sync().await.unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.task_result(task_id).unwrap()["status"], "CANCELLED");
    assert!(state.leases.is_empty());
    assert_eq!(entries(&f).await[0]["status"], "CANCELLED");
    assert!(
        f.service
            .execute(
                json!({"command":"plan.create","task":task_id,"body":"# Stale"}),
                owner(&owned)
            )
            .await
            .is_err()
    );
    let text = std::fs::read_to_string(path(&f)).unwrap();
    let start = text.find("- [-] Deliver").unwrap();
    let end = text.find(END).unwrap();
    std::fs::write(path(&f), format!("{}{}", &text[..start], &text[end..])).unwrap();
    f.service.sync().await.unwrap();
    let rows = entries(&f).await;
    assert_eq!(rows[0]["id"], entry["id"]);
    assert_eq!(rows[0]["deleted"], true);
    assert!(
        !std::fs::read_to_string(path(&f))
            .unwrap()
            .contains("Deliver feature")
    );
    assert!(
        f.service
            .store()
            .snapshot()
            .await
            .unwrap()
            .jobs
            .iter()
            .any(|j| j.id == job)
    );
}

#[tokio::test]
async fn inbox_expiry_resumes_the_same_job_and_never_revives_cancellation() {
    let f = fixture().await;
    add(&f, "Resume me").await;
    let first = claim(&f, "one").await;
    f.clock.fetch_add(901, Ordering::SeqCst);
    let second = claim(&f, "two").await;
    assert_eq!(second["claimed"], true);
    assert_eq!(first["job"]["id"], second["job"]["id"]);
    assert_ne!(
        first["entry"]["lease"]["token"],
        second["entry"]["lease"]["token"]
    );
    let id = second["entry"]["id"].as_str().unwrap();
    f.service
        .execute(
            json!({"command":"inbox.cancel","inbox":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.clock.fetch_add(901, Ordering::SeqCst);
    assert_eq!(claim(&f, "three").await["claimed"], false);
    assert_eq!(entries(&f).await[0]["status"], "CANCELLED");
}

#[tokio::test]
async fn inbox_missing_or_invalid_document_does_not_cancel_all_entries() {
    let f = fixture().await;
    add(&f, "Keep me").await;
    let source = std::fs::read_to_string(path(&f)).unwrap();
    for text in ["", "# Not an Inbox"] {
        std::fs::write(path(&f), text).unwrap();
        assert!(f.service.sync().await.is_err());
        assert_eq!(entries(&f).await[0]["status"], "TODO");
    }
    std::fs::remove_file(path(&f)).unwrap();
    assert!(f.service.sync().await.is_err());
    assert_eq!(entries(&f).await[0]["status"], "TODO");
    std::fs::write(path(&f), source).unwrap();
    f.service.sync().await.unwrap();
}

#[tokio::test]
async fn inbox_completion_checks_the_box_and_idempotent_append_keeps_one_entry() {
    let f = fixture().await;
    let request = json!({"command":"inbox.add","project":f.project,"content":"Ship\n\n  Preserve indentation"});
    let options = WriteOptions {
        idempotency_key: Some("im:message:1".into()),
        ..WriteOptions::default()
    };
    let first = f
        .service
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    let replay = f.service.execute(request, options).await.unwrap();
    assert_eq!(first.result["id"], replay.result["id"]);
    assert_eq!(entries(&f).await.len(), 1);
    let claimed = claim(&f, "one").await;
    let t = f
        .service
        .execute(
            json!({"command":"task.add","job":claimed["job"]["id"],"title":"Ship"}),
            identity("one"),
        )
        .await
        .unwrap()
        .result;
    let owned = f.start(t["id"].as_str().unwrap(), "one").await;
    f.service
        .execute(json!({"command":"task.done","task":t["id"]}), owner(&owned))
        .await
        .unwrap();
    assert_eq!(entries(&f).await[0]["status"], "PENDING_REVIEW");
    assert!(entries(&f).await[0]["lease"].is_null());
    assert_eq!(claim(&f, "other").await["claimed"], false);
    f.service.execute(json!({"command":"job.reject","job":claimed["job"]["id"],"reason":"Needs another review"}), WriteOptions::default()).await.unwrap();
    assert_eq!(entries(&f).await[0]["status"], "ACTIVE");
    f.service
        .execute(
            json!({"command":"job.submit","job":claimed["job"]["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(entries(&f).await[0]["status"], "PENDING_REVIEW");
    f.service
        .execute(
            json!({"command":"job.approve","job":claimed["job"]["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(entries(&f).await[0]["status"], "COMPLETED");
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains(&format!(
        "- [x] Ship <!-- taskcli:entry:{} --> <!-- taskcli:entry-state COMPLETED revision={} --> · ",
        first.result["id"].as_str().unwrap(), entries(&f).await[0]["revision"]
            ))
    );
}

#[tokio::test]
async fn inbox_deleted_active_entry_cancels_and_cannot_be_restored_by_an_old_buffer() {
    let f = fixture().await;
    add(&f, "Withdraw").await;
    let claimed = claim(&f, "one").await;
    let source = std::fs::read_to_string(path(&f)).unwrap();
    let start = source.find("- [/] Withdraw").unwrap();
    let end = source.find(END).unwrap();
    let removed = format!("{}{}", &source[..start], &source[end..]);
    std::fs::write(path(&f), &removed).unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(entries(&f).await[0]["status"], "CANCELLED");
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(
        state
            .jobs
            .iter()
            .find(|j| j.id == claimed["job"]["id"])
            .unwrap()
            .status,
        agentix_task::JobStatus::Cancelled
    );
    std::fs::write(path(&f), source).unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(std::fs::read_to_string(path(&f)).unwrap(), removed);
}

#[tokio::test]
async fn inbox_unpublished_append_survives_restart_and_manual_append() {
    let f = fixture().await;
    let pending = f
        .service
        .store()
        .execute(
            json!({"command":"inbox.add","project":f.project,"content":"Pending delivery"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(pending["published"], false);
    let source = std::fs::read_to_string(path(&f))
        .unwrap()
        .replace(END, &format!("- [ ] Human append\n\n{END}"));
    std::fs::write(path(&f), source).unwrap();
    let restored = Service::open(f.service.config().clone()).await.unwrap();
    restored.sync().await.unwrap();
    let text = std::fs::read_to_string(path(&f)).unwrap();
    assert!(text.contains("Human append") && text.contains("Pending delivery"));
    assert_eq!(entries(&f).await.len(), 2);
    assert!(
        entries(&f)
            .await
            .iter()
            .all(|e| e["status"] == "TODO" && e["deleted"] == false)
    );
}

#[tokio::test]
async fn inbox_id_reordering_keeps_identity_and_duplicate_ids_reject_sync() {
    let f = fixture().await;
    let a = add(&f, "Alpha").await;
    let b = add(&f, "Beta").await;
    let source = std::fs::read_to_string(path(&f)).unwrap();
    let first = source.find("- [ ] Alpha").unwrap();
    let second = source.find("- [ ] Beta").unwrap();
    let end = source.find(END).unwrap();
    let reordered = format!(
        "{}{}{}{}",
        &source[..first],
        &source[second..end],
        &source[first..second],
        &source[end..]
    );
    std::fs::write(path(&f), &reordered).unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(claim(&f, "one").await["entry"]["id"], b["id"]);
    let duplicate = reordered.replace(a["id"].as_str().unwrap(), b["id"].as_str().unwrap());
    std::fs::write(path(&f), duplicate).unwrap();
    assert!(f.service.sync().await.is_err());
    assert_eq!(entries(&f).await.len(), 2);
}

#[tokio::test]
async fn inbox_projection_has_no_blank_lines_between_items() {
    let f = Fixture::new().await;
    for content in ["First\n\nParagraph", "Second"] {
        f.service
            .execute(
                json!({"command":"inbox.add","project":f.project,"content":content}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Inbox.md");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("\n\n- [ ] Second"), "{text}");
    assert!(
        text.contains("\n\n  Paragraph\n- [ ] Second"),
        "preserve authored paragraph breaks: {text}"
    );
    f.service.sync().await.unwrap();
    assert_eq!(text, std::fs::read_to_string(path).unwrap());
}

#[tokio::test]
async fn inbox_source_edit_survives_projection_interruption_without_reverting_content() {
    let f = Fixture::new().await;
    let opts = WriteOptions {
        actor_ref: "im:owner".into(),
        ..WriteOptions::default()
    };
    let added = f.service.execute(json!({"command":"inbox.add","project":f.project,"content":"Old","source":"message-key"}), opts.clone()).await.unwrap().result;
    f.service.store().execute(json!({"command":"inbox.edit","inbox":added["id"],"content":"New\n\nParagraph","source":"message-key","version":2}), opts.clone()).await.unwrap();
    f.service.sync().await.unwrap();
    let entry = &f.service.store().snapshot().await.unwrap().inboxes[0];
    assert_eq!(entry.content, "New\n\nParagraph");
    assert_eq!(entry.id, added["id"]);
    let text = std::fs::read_to_string(
        f.service
            .config()
            .output_dir()
            .join("Projects/demo/Inbox.md"),
    )
    .unwrap();
    assert!(text.contains("- [ ] New"));
    f.service.sync().await.unwrap();
    assert_eq!(
        entry,
        &f.service.store().snapshot().await.unwrap().inboxes[0]
    );
    for content in ["", "<!-- taskcli:inbox:end -->"] {
        assert!(f.service.execute(json!({"command":"inbox.edit","inbox":added["id"],"content":content,"source":"message-key","version":3}), opts.clone()).await.is_err());
    }
}

#[tokio::test]
async fn inbox_source_migration_recovers_legacy_im_associations() {
    let f = Fixture::new().await;
    let options = WriteOptions {
        actor_ref: "im:owner".into(),
        idempotency_key: Some("im:inbox:[\"feishu\",\"chat\",\"message\"]".into()),
        ..WriteOptions::default()
    };
    f.service
        .execute(
            json!({"command":"inbox.add","project":f.project,"content":"Legacy"}),
            options,
        )
        .await
        .unwrap();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(&f.service.config().storage.path),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE inbox_entries SET data = json_remove(data, '$.source')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version = 10")
        .execute(&pool)
        .await
        .unwrap();
    let store = Store::open(&f.service.config().storage.path).await.unwrap();
    assert_eq!(
        serde_json::to_value(&store.snapshot().await.unwrap().inboxes[0]).unwrap()["source"],
        "[\"feishu\",\"chat\",\"message\"]"
    );
}

#[tokio::test]
async fn selected_inboxes_follow_job_lifecycle() {
    for policy in ["none", "required"] {
        let mut f = fixture().await;
        // A fresh human entry must be imported before matching the prompt.
        let doc = std::fs::read_to_string(path(&f)).unwrap();
        std::fs::write(
            path(&f),
            doc.replace(END, &format!("- [ ] Fix login error\n{END}")),
        )
        .unwrap();
        let candidate = f
            .service
            .execute(
                json!({"command":"inbox.list","project":f.project}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result[0]
            .clone();
        let second = add(&f, "Add login regression coverage").await;
        let job = f
            .service
            .execute(
                json!({"command":"job.create","project":f.project,"title":"Login fix",
                "prompt":"修复登录并增加回归测试", "inbox_ids":[candidate["id"], second["id"]], "review_policy":policy}),
                identity("worker"),
            )
            .await
            .unwrap()
            .result;
        f.job = job["id"].as_str().unwrap().into();
        let linked = entries(&f).await;
        assert_eq!(linked.len(), 2);
        assert!(
            linked
                .iter()
                .all(|e| e["job_id"] == f.job && e["status"] == "ACTIVE")
        );
        assert_eq!(linked[0]["job_id"], f.job);
        assert_eq!(linked[0]["status"], "ACTIVE");
        assert_eq!(linked[0]["last_session"], "worker");
        assert!(
            std::fs::read_to_string(path(&f))
                .unwrap()
                .contains("- [/] Fix login error")
        );
        let task = f.task("Fix login").await;
        let owned = f.start(&task, "worker").await;
        f.service
            .execute(json!({"command":"task.done","task":task}), owner(&owned))
            .await
            .unwrap();
        if policy == "required" {
            assert_eq!(entries(&f).await[0]["status"], "PENDING_REVIEW");
            f.approve().await;
        }
        assert!(entries(&f).await.iter().all(|e| e["status"] == "COMPLETED"));
        assert!(
            std::fs::read_to_string(path(&f))
                .unwrap()
                .contains("- [x] Fix login error")
        );
    }
}

#[tokio::test]
async fn prompt_text_alone_never_links_inbox() {
    let f = fixture().await;
    add(&f, "Fix login").await;
    let before = entries(&f).await;
    f.service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Delivery",
            "prompt":"Please Fix login"}),
            identity("worker"),
        )
        .await
        .unwrap();
    assert_eq!(entries(&f).await, before);
}

#[tokio::test]
async fn inbox_selection_is_atomic_and_rejects_unavailable_entries() {
    let f = fixture().await;
    let available = add(&f, "Fix login").await;
    let unavailable = add(&f, "Closed work").await;
    for status in ["ACTIVE", "PENDING_REVIEW", "COMPLETED", "TODO", "CANCELLED"] {
        set_status(&f, unavailable["id"].as_str().unwrap(), status)
            .await
            .unwrap();
        if status == "TODO" {
            continue;
        }
        let before = f.service.store().snapshot().await.unwrap();
        let result = f
            .service
            .execute(
                json!({"command":"job.create","project":f.project,
            "title":"Delivery","prompt":"Fix both issues",
            "inbox_ids":[available["id"], unavailable["id"]]}),
                identity("worker"),
            )
            .await;
        assert!(result.is_err(), "must reject {status}");
        let after = f.service.store().snapshot().await.unwrap();
        assert_eq!(after.inboxes, before.inboxes);
        assert_eq!(after.jobs, before.jobs);
    }
    for selection in [
        json!([available["id"], "inbox_missing"]),
        json!([42]),
        json!("bad"),
    ] {
        assert!(
            f.service
                .execute(
                    json!({"command":"job.create","project":f.project,
            "title":"Delivery","prompt":"Fix issues","inbox_ids":selection}),
                    identity("worker")
                )
                .await
                .is_err()
        );
    }
    let other = f
        .service
        .execute(
            json!({"command":"project.register","name":"other",
        "root":f.dir.path().join("other")}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert!(
        f.service
            .execute(
                json!({"command":"job.create","project":other["id"],
        "title":"Other delivery","prompt":"Fix issues","inbox_ids":[available["id"]]}),
                identity("worker")
            )
            .await
            .is_err()
    );
    f.service
        .execute(
            json!({"command":"job.create","project":f.project,
        "title":"Delivery","prompt":"修复登录","inbox_ids":[available["id"]]}),
            identity("worker"),
        )
        .await
        .unwrap();
    let before = entries(&f).await;
    assert!(
        f.service
            .execute(
                json!({"command":"job.create","project":f.project,
        "title":"Steal","prompt":"修复登录","inbox_ids":[available["id"]]}),
                identity("other")
            )
            .await
            .is_err()
    );
    assert_eq!(entries(&f).await, before);
}

#[tokio::test]
async fn prompt_links_on_backfill_and_followup() {
    let mut f = fixture().await;
    f.job = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Work"}),
            identity("worker"),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .into();
    let first = add(&f, "First request").await;
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"prompt":"处理第一个需求","inbox_ids":[first["id"]]}),
            identity("worker"),
        )
        .await
        .unwrap();
    assert_eq!(entries(&f).await[0]["job_id"], f.job);
    let task = f.task("First delivery").await;
    let owned = f.start(&task, "worker").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&owned))
        .await
        .unwrap();
    let second = add(&f, "Second request").await;
    f.service
        .execute(
            json!({"command":"job.followup","job":f.job,"prompt":"也处理第二个需求","inbox_ids":[second["id"]]}),
            identity("worker"),
        )
        .await
        .unwrap();
    let linked = entries(&f).await;
    assert!(
        linked
            .iter()
            .all(|e| e["job_id"] == f.job && e["status"] == "ACTIVE")
    );
}

#[tokio::test]
async fn session_exit_keeps_another_hosts_same_id_inbox_lease() {
    let f = fixture().await;
    add(&f, "Keep the Codex claim").await;
    let options = WriteOptions {
        actor_ref: "agent:codex".into(),
        session_ref: Some("same-id".into()),
        ..WriteOptions::default()
    };
    f.service
        .execute(
            json!({"command":"inbox.claim-next","project":f.project}),
            options,
        )
        .await
        .unwrap();
    assert!(
        f.service.store().snapshot().await.unwrap().inboxes[0]
            .lease
            .is_some()
    );
    f.service
        .execute(
            json!({"command":"session.end","session":"same-id","executor":"agent:pi"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        f.service.store().snapshot().await.unwrap().inboxes[0]
            .lease
            .is_some()
    );
    f.service
        .execute(
            json!({"command":"session.end","session":"same-id","executor":"agent:codex"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        f.service.store().snapshot().await.unwrap().inboxes[0]
            .lease
            .is_none()
    );
}
