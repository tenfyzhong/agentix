use super::*;

const END: &str = "<!-- taskcli:inbox:end -->";

#[tokio::test]
async fn inbox_rejects_reserved_control_markers_before_committing_a_submission() {
    let f = fixture("markdown").await;
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
    let f = fixture("markdown").await;
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

async fn fixture(format: &str) -> Fixture {
    let f = Fixture::new(format).await;
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
    for format in ["markdown", "obsidian"] {
        let f = fixture(format).await;
        let content = "Request\nDetails with **Markdown**.\n- [ ] Acceptance";
        let entry = add(&f, content).await;
        let id = entry["id"].as_str().unwrap();
        let source = std::fs::read_to_string(path(&f)).unwrap();
        assert!(source.contains(&format!(
            "- [ ] Request <!-- taskcli:entry:{id} --> <!-- taskcli:entry-state TODO -->\n  Details with **Markdown**.\n  - [ ] Acceptance\n\n"
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
            .find(|line| line.starts_with("- [ ] Request"))
            .unwrap();
        assert!(header.contains("<!-- taskcli:entry-state IN_PROGRESS --> · "));
        assert!(header.contains(if format == "obsidian" { "[[" } else { "](" }));
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
        assert!(source.contains(&format!(
            "- [-] Request <!-- taskcli:entry:{id} --> <!-- taskcli:entry-state CANCELLED --> · "
        )));
        assert!(!source.contains("agent:one"));
    }
}

#[tokio::test]
async fn inbox_legacy_receipt_migrates_without_changing_identity_or_content() {
    let f = fixture("markdown").await;
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
    assert!(source.contains(&format!(
        "- [ ] Request <!-- taskcli:entry:{id} --> <!-- taskcli:entry-state TODO -->\n  Details.\n\n"
    )));
    f.service.sync().await.unwrap();
    assert_eq!(entries(&f).await, rows);
    assert_eq!(std::fs::read_to_string(path(&f)).unwrap(), source);
}

#[tokio::test]
async fn inbox_import_preserves_markdown_and_ignores_nested_and_fenced_checklists() {
    for format in ["markdown", "obsidian"] {
        let f = fixture(format).await;
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
}

#[tokio::test]
async fn inbox_claim_is_exclusive_and_waits_for_all_project_jobs() {
    let f = Fixture::new("markdown").await;
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
            .filter(|r| r["status"] == "IN_PROGRESS")
            .count(),
        1
    );
}

#[tokio::test]
async fn inbox_cancellation_revokes_task_ownership_and_deletion_preserves_history() {
    let f = fixture("markdown").await;
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
        .replace("- [ ] Deliver", "- [-] Deliver");
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
    let f = fixture("markdown").await;
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
    let f = fixture("markdown").await;
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
    let f = fixture("obsidian").await;
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
    assert_eq!(entries(&f).await[0]["status"], "DONE");
    assert!(
        std::fs::read_to_string(path(&f))
            .unwrap()
            .contains(&format!(
                "- [x] Ship <!-- taskcli:entry:{} --> <!-- taskcli:entry-state DONE --> · ",
                first.result["id"].as_str().unwrap()
            ))
    );
}

#[tokio::test]
async fn inbox_deleted_active_entry_cancels_and_cannot_be_restored_by_an_old_buffer() {
    let f = fixture("markdown").await;
    add(&f, "Withdraw").await;
    let claimed = claim(&f, "one").await;
    let source = std::fs::read_to_string(path(&f)).unwrap();
    let start = source.find("- [ ] Withdraw").unwrap();
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
    let f = fixture("markdown").await;
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
    let f = fixture("markdown").await;
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
