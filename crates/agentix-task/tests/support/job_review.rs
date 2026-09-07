use super::*;

#[tokio::test]
async fn job_review_migrates_schema_eight_without_reopening_historical_completed_jobs() {
    let f = Fixture::new("markdown").await;
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(&f.service.config().storage.path),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET data = json_remove(json_set(data, '$.status', 'COMPLETED', '$.completed_at', 123), '$.review_reason')")
        .execute(&pool).await.unwrap();
    sqlx::query("PRAGMA user_version = 8")
        .execute(&pool)
        .await
        .unwrap();
    let store = Store::open(&f.service.config().storage.path).await.unwrap();
    let state = store.snapshot().await.unwrap();
    assert_eq!(state.jobs[0].status.to_string(), "COMPLETED");
    assert_eq!(state.jobs[0].completed_at, Some(123));
    assert!(state.jobs[0].review_reason.is_none());
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 12);
}

async fn finish(f: &Fixture, id: &str) {
    let claim = f.start(id, "review-test").await;
    f.service
        .execute(json!({"command":"task.done","task":id}), owner(&claim))
        .await
        .unwrap();
}

async fn job(f: &Fixture) -> Value {
    serde_json::to_value(&f.service.store().snapshot().await.unwrap().jobs[0]).unwrap()
}

async fn change(f: &Fixture, command: &str) -> Value {
    f.service
        .execute(
            json!({"command":command,"job":f.job,"reason":"Acceptance failed"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result
}

#[tokio::test]
async fn job_review_requires_human_approval_and_preserves_rejected_work() {
    let f = Fixture::new("obsidian").await;
    let id = f.task("Implement").await;
    finish(&f, &id).await;
    assert_eq!(job(&f).await["status"], "PENDING_REVIEW");
    assert!(job(&f).await["completed_at"].is_null());
    assert_eq!(change(&f, "job.reject").await["status"], "ACTIVE");
    assert_eq!(job(&f).await["review_reason"], "Acceptance failed");
    assert_eq!(
        f.service.store().snapshot().await.unwrap().tasks[0]
            .status
            .to_string(),
        "DONE"
    );
    f.service
        .execute(
            json!({"command":"task.update","task":id,"name":"Renamed"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(job(&f).await["status"], "ACTIVE");
    assert_eq!(change(&f, "job.submit").await["status"], "PENDING_REVIEW");
    assert!(job(&f).await["review_reason"].is_null());
    assert_eq!(change(&f, "job.approve").await["status"], "COMPLETED");
    assert!(job(&f).await["completed_at"].is_number());
    let events = f
        .service
        .store()
        .events(Some(&f.job), 0, 100)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "job.pending_review")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "job.completed")
            .count(),
        1
    );
    assert!(events.iter().any(
        |e| e.event_type == "job.rejected" && e.payload["review_reason"] == "Acceptance failed"
    ));
}

#[tokio::test]
async fn job_review_rejects_incomplete_work_and_nonterminal_archival() {
    let f = Fixture::new("markdown").await;
    for command in ["job.submit", "job.approve", "job.reject"] {
        assert!(
            f.service
                .execute(
                    json!({"command":command,"job":f.job,"reason":"No work"}),
                    WriteOptions::default()
                )
                .await
                .is_err()
        );
    }
    let id = f.task("Unfinished").await;
    assert!(
        f.service
            .execute(
                json!({"command":"job.submit","job":f.job}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    finish(&f, &id).await;
    for request in [
        json!({"command":"job.archive","job":f.job}),
        json!({"command":"project.archive","project":f.project}),
    ] {
        assert!(
            f.service
                .execute(request, WriteOptions::default())
                .await
                .is_err()
        );
    }
    assert!(
        f.service
            .execute(
                json!({"command":"job.reject","job":f.job,"reason":" "}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    assert_eq!(change(&f, "job.cancel").await["status"], "CANCELLED");
    change(&f, "job.archive").await;
}

#[tokio::test]
async fn job_review_rework_must_be_finished_again_and_all_cancelled_is_not_delivery() {
    let f = Fixture::new("markdown").await;
    let id = f.task("Rework").await;
    finish(&f, &id).await;
    change(&f, "job.reject").await;
    f.service
        .execute(
            json!({"command":"task.reopen","task":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(job(&f).await["review_reason"], "Acceptance failed");
    finish(&f, &id).await;
    assert_eq!(job(&f).await["status"], "PENDING_REVIEW");
    assert!(job(&f).await["review_reason"].is_null());
    f.service
        .execute(
            json!({"command":"task.reopen","task":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(job(&f).await["status"], "ACTIVE");
    f.service
        .execute(
            json!({"command":"task.cancel","task":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(job(&f).await["status"], "ACTIVE");
    assert!(
        f.service
            .execute(
                json!({"command":"job.submit","job":f.job}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn job_review_serializes_competing_decisions_and_replays_approval() {
    let f = Fixture::new("markdown").await;
    let id = f.task("Review once").await;
    finish(&f, &id).await;
    let revision = job(&f).await["revision"].as_i64().unwrap();
    let options = WriteOptions {
        expected_revision: Some(revision),
        idempotency_key: Some("approve-once".into()),
        ..WriteOptions::default()
    };
    let request = json!({"command":"job.approve","job":f.job});
    let first = f
        .service
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    let replay = f.service.execute(request, options).await.unwrap();
    assert_eq!(first.sequence, replay.sequence);
    let error = f
        .service
        .execute(
            json!({"command":"job.reject","job":f.job,"reason":"Stale review"}),
            WriteOptions {
                expected_revision: Some(revision),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("revision"));
    assert_eq!(job(&f).await["status"], "COMPLETED");
}

#[tokio::test]
async fn review_time_tracks_each_submission_and_projects_local_dates() {
    let f = Fixture::new("obsidian").await;
    assert!(job(&f).await["pending_review_at"].is_null());
    let id = f.task("Timed review").await;
    finish(&f, &id).await;
    let first = f.clock.load(Ordering::SeqCst);
    assert_eq!(job(&f).await["pending_review_at"], first);
    f.clock.fetch_add(60, Ordering::SeqCst);
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"name":"Edited"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(job(&f).await["pending_review_at"], first);
    change(&f, "job.reject").await;
    f.clock.fetch_add(60, Ordering::SeqCst);
    change(&f, "job.submit").await;
    let second = f.clock.load(Ordering::SeqCst);
    assert_eq!(job(&f).await["pending_review_at"], second);
    change(&f, "job.approve").await;
    assert_eq!(job(&f).await["pending_review_at"], second);
    let value = job(&f).await;
    let doc = std::fs::read_to_string(
        f.service
            .config()
            .output_dir()
            .join(value["document_path"].as_str().unwrap()),
    )
    .unwrap();
    let props: Value = serde_yaml::from_str(
        doc.strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0,
    )
    .unwrap();
    let snapshot = f.service.obsidian_snapshot().await.unwrap();
    let note = snapshot["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == f.job)
        .unwrap();
    for field in [
        "created_at",
        "updated_at",
        "completed_at",
        "pending_review_at",
    ] {
        let instant =
            time::OffsetDateTime::from_unix_timestamp(value[field].as_i64().unwrap()).unwrap();
        let local = instant
            .to_offset(time::UtcOffset::local_offset_at(instant).unwrap())
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        assert_eq!(props[field], local, "{field}");
        if field != "created_at" {
            assert_eq!(note["properties"][field], local);
        }
    }
}

#[tokio::test]
async fn review_time_migration_uses_submission_event_before_later_edits() {
    let f = Fixture::new("markdown").await;
    let id = f.task("Legacy review").await;
    finish(&f, &id).await;
    let submitted = f.clock.load(Ordering::SeqCst);
    f.clock.fetch_add(60, Ordering::SeqCst);
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"name":"Later edit"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(&f.service.config().storage.path),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET data = json_remove(data, '$.pending_review_at')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version = 10")
        .execute(&pool)
        .await
        .unwrap();
    let store = Store::open(&f.service.config().storage.path).await.unwrap();
    let state = store.snapshot().await.unwrap();
    assert_eq!(
        serde_json::to_value(&state.jobs[0]).unwrap()["pending_review_at"],
        submitted
    );
    let again = Store::open(&f.service.config().storage.path).await.unwrap();
    assert_eq!(state.jobs, again.snapshot().await.unwrap().jobs);
}

#[tokio::test]
async fn followup_preserves_job_and_snapshots_previous_dependencies() {
    let f = Fixture::new("markdown").await;
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"prompt":"Original request"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let old = f.task("Original work").await;
    finish(&f, &old).await;
    let result = f
        .service
        .execute(
            json!({"command":"job.followup","job":f.job,"prompt":"Also handle this\nverbatim"}),
            WriteOptions {
                session_ref: Some("followup-session".into()),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap()
        .result;
    assert_eq!(result["status"], "ACTIVE");
    assert_eq!(result["prompt"], "Original request");
    assert_eq!(result["followup_task_ids"], json!([old]));
    assert_eq!(
        result["conversation"][0]["text"],
        "Also handle this\nverbatim"
    );
    let a = f.task("First followup").await;
    let b = f.task("Second followup").await;
    let state = f.service.store().snapshot().await.unwrap();
    for id in [&a, &b] {
        assert_eq!(
            state.tasks[state.task_index(id).unwrap()].dependencies,
            vec![old.clone()]
        );
    }
    finish(&f, &a).await;
    assert_eq!(job(&f).await["status"], "ACTIVE");
    finish(&f, &b).await;
    assert_eq!(job(&f).await["status"], "PENDING_REVIEW");
    f.service
        .execute(
            json!({"command":"job.followup","job":f.job,"prompt":"Third request"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let third = f.task("Third delivery").await;
    let state = f.service.store().snapshot().await.unwrap();
    let deps = &state.tasks[state.task_index(&third).unwrap()].dependencies;
    assert_eq!(deps.len(), 3);
    for prior in [&old, &a, &b] {
        assert!(deps.contains(prior));
    }
    assert_eq!(state.jobs[0].conversation.len(), 2);
    assert_eq!(state.jobs[0].prompt, "Original request");
}

#[tokio::test]
async fn review_policy_none_completes_only_finished_work() {
    let f = Fixture::new("markdown").await;
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"review_policy":"none"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let id = f.task("Investigate").await;
    assert!(
        f.service
            .execute(
                json!({"command":"job.submit","job":f.job}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    finish(&f, &id).await;
    assert_eq!(job(&f).await["status"], "COMPLETED");
    assert!(job(&f).await["completed_at"].is_number());
    assert!(job(&f).await["pending_review_at"].is_null());
}

#[tokio::test]
async fn followup_cancelled_prerequisite_stays_cancelled_and_blocks_execution() {
    let f = Fixture::new("markdown").await;
    let cancelled = f.task("Cancelled work").await;
    f.service
        .execute(
            json!({"command":"task.cancel","task":cancelled}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let done = f.task("Delivered work").await;
    finish(&f, &done).await;
    f.service
        .execute(
            json!({"command":"job.followup","job":f.job,"prompt":"More work"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let id = f.task("Supplement").await;
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(
        state.tasks[state.task_index(&cancelled).unwrap()].status,
        agentix_task::TaskStatus::Cancelled
    );
    assert!(
        state.tasks[state.task_index(&id).unwrap()]
            .dependencies
            .contains(&cancelled)
    );
    let claim = f.claim(&id, "followup").await;
    f.plan(&id).await;
    let error = f
        .service
        .execute(json!({"command":"task.start","task":id}), owner(&claim))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("dependencies are incomplete"));
}

#[tokio::test]
async fn review_policy_creation_validation_and_explicit_submit() {
    let f = Fixture::new("markdown").await;
    assert_eq!(job(&f).await["review_policy"], "required");
    let created = f.service.execute(json!({"command":"job.create","project":f.project,"title":"Documentation","review_policy":"none"}), WriteOptions::default()).await.unwrap().result;
    assert_eq!(created["review_policy"], "none");
    assert!(
        f.service
            .execute(
                json!({"command":"job.update","job":f.job,"review_policy":"invalid"}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    let id = f.task("Delivery").await;
    finish(&f, &id).await;
    change(&f, "job.reject").await;
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"review_policy":"none"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(change(&f, "job.submit").await["status"], "COMPLETED");
}

#[tokio::test]
async fn followup_new_session_records_before_tasks_without_stealing_later_capture() {
    let f = Fixture::new("markdown").await;
    let id = f.task("Original delivery").await;
    finish(&f, &id).await;
    let options = WriteOptions {
        session_ref: Some("new-session".into()),
        ..WriteOptions::default()
    };
    f.service.execute(json!({"command":"job.create","project":f.project,"title":"Earlier same-second request"}), options.clone()).await.unwrap();
    f.service
        .execute(
            json!({"command":"job.followup","job":f.job,"prompt":"New session supplement"}),
            options.clone(),
        )
        .await
        .unwrap();
    for (message, explicit) in [("automatic", false), ("explicit", true)] {
        let mut request = json!({"command":"session.record","session":"new-session","messages":[{"id":message,"role":"assistant","text":message}]});
        if explicit {
            request["job"] = json!(f.job);
        }
        let recorded = f
            .service
            .execute(request, options.clone())
            .await
            .unwrap()
            .result;
        assert_eq!(recorded["recorded"], 1);
        assert_eq!(recorded["job_id"], f.job);
    }
    let next = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Separate later request"}),
            options.clone(),
        )
        .await
        .unwrap()
        .result;
    f.service.execute(json!({"command":"session.record","session":"new-session","job":f.job,"messages":[{"id":"late","role":"assistant","text":"Late old result"}]}), options.clone()).await.unwrap();
    f.service.execute(json!({"command":"session.record","session":"new-session","messages":[{"id":"new","role":"assistant","text":"New result"}]}), options).await.unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(
        state.jobs[state.job_index(next["id"].as_str().unwrap()).unwrap()].conversation[0].text,
        "New result"
    );
}
