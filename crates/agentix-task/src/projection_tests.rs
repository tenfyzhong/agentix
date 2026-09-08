use super::*;

thread_local! {
    pub(super) static FRONTMATTER_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

async fn export_fixture() -> (tempfile::TempDir, Service, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let service = Service::open(Config {
        schema_version: 1,
        storage: crate::StorageConfig {
            path: dir.path().join("tasks.sqlite3"),
        },
        documents: crate::DocumentConfig {
            root: dir.path().to_owned(),
            directory: "notes".into(),
        },
    })
    .await
    .unwrap();
    let project = service
        .store
        .execute(
            json!({"command":"project.register","name":"Export","root":dir.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let job = service
        .store
        .execute(
            json!({"command":"job.create","project":project["id"],"title":"Export"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let task = service
        .store
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Task"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    (dir, service, task["id"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn obsidian_export_does_not_generate_markdown_for_properties() {
    let (_dir, service, task) = export_fixture().await;
    FRONTMATTER_CALLS.set(0);
    let snapshot = service.obsidian_snapshot().await.unwrap();
    assert_eq!(
        FRONTMATTER_CALLS.get(),
        0,
        "property exports should not render and parse Markdown"
    );
    let point = service.obsidian_note(&task).await.unwrap();
    assert_eq!(snapshot["notes"][0], point);
}

#[tokio::test]
async fn obsidian_export_ignores_authored_bodies() {
    let (_dir, service, _) = export_fixture().await;
    let project = service.store.projects().await.unwrap()[0].id.clone();
    service
        .store
        .execute(
            json!({"command":"inbox.add","project":project,"content":"Submission"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE inbox_entries SET data = json_set(data, '$.published', json('true'))")
        .execute(&service.store.pool)
        .await
        .unwrap();
    let expected = service.obsidian_snapshot().await.unwrap();
    assert_eq!(expected["notes"].as_array().unwrap().len(), 3);
    // Valid JSON with deliberately incompatible body types makes accidental
    // full-entity deserialization fail, without relying on wall-clock timing.
    for query in [
        "UPDATE tasks SET data = json_set(data, '$.title', json('{}'))",
        "UPDATE jobs SET data = json_set(data, '$.title', json('{}'), '$.goal', json('{}'), '$.prompt', json('{}'), '$.conversation', json('{}'))",
        "UPDATE inbox_entries SET data = json_set(data, '$.content', json('{}'), '$.lease', json('{}'), '$.source', json('{}'))",
    ] {
        sqlx::query(query)
            .execute(&service.store.pool)
            .await
            .unwrap();
    }
    assert_eq!(service.obsidian_snapshot().await.unwrap(), expected);
}

#[tokio::test]
async fn obsidian_export_normalizes_legacy_inbox_status() {
    let (_dir, service, _) = export_fixture().await;
    let project = service.store.projects().await.unwrap()[0].id.clone();
    let inbox = service
        .store
        .execute(
            json!({"command":"inbox.add","project":project,"content":"Submission"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    sqlx::query("UPDATE inbox_entries SET data = json_set(data, '$.published', json('true'), '$.status', 'IN_PROGRESS')")
        .execute(&service.store.pool).await.unwrap();
    let snapshot = service.obsidian_snapshot().await.unwrap();
    assert_eq!(snapshot["notes"][2]["status"], "ACTIVE");
    assert_eq!(snapshot["notes"][2]["properties"]["status"], "ACTIVE");
    assert_eq!(
        snapshot["notes"][2],
        service
            .obsidian_note(inbox["id"].as_str().unwrap())
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn obsidian_metadata_payload_does_not_grow_with_job_conversation() {
    let (_dir, service, _) = export_fixture().await;
    let expected = service.store.obsidian_records().await.unwrap();
    let conversation = json!([{"user_input":"x".repeat(1_000_000)}]).to_string();
    sqlx::query("UPDATE jobs SET data = json_set(data, '$.conversation', json(?))")
        .bind(conversation)
        .execute(&service.store.pool)
        .await
        .unwrap();
    assert_eq!(service.store.obsidian_records().await.unwrap(), expected);
}

#[tokio::test]
async fn obsidian_export_ignores_plan_and_lease_bodies() {
    let (_dir, service, task) = export_fixture().await;
    let expected = service.obsidian_snapshot().await.unwrap();
    sqlx::query("INSERT INTO plans(id,data) VALUES('plan_unrelated','{}')")
        .execute(&service.store.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO task_leases(id,data) VALUES(?,'{}')")
        .bind(&task)
        .execute(&service.store.pool)
        .await
        .unwrap();
    assert_eq!(service.obsidian_snapshot().await.unwrap(), expected);
}

#[tokio::test]
async fn full_projection_does_not_rescan_entity_vectors_for_each_document() {
    let (_dir, service, _) = export_fixture().await;
    let project = service.store.projects().await.unwrap()[0].id.clone();
    for index in 0..64 {
        let job = service
            .store
            .execute(
                json!({"command":"job.create","project":project,"title":format!("Job {index}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        service
            .store
            .execute(
                json!({"command":"task.add","job":job["id"],"title":format!("Task {index}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let state = service.store.snapshot().await.unwrap();
    crate::model::RESOLVE_ITEMS.set(0);
    service
        .render_state_locked(&state, None, &BTreeMap::new())
        .await
        .unwrap();
    let visited = crate::model::RESOLVE_ITEMS.get();
    assert!(
        visited < state.tasks.len() * 20,
        "full projection scanned {visited} entity identifiers for {} tasks",
        state.tasks.len()
    );
}

async fn planned_export_fixture() -> (tempfile::TempDir, Service, String) {
    let (dir, service, task) = export_fixture().await;
    let claim = service
        .store
        .execute(
            json!({"command":"task.claim","task":task,"executor":"agent:test","session":"export"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    service
        .execute(
            json!({"command":"plan.create","task":task,"body":"Initial plan"}),
            WriteOptions {
                session_ref: Some("export".into()),
                lease_token: Some(claim["lease"]["token"].as_str().unwrap().into()),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap();
    (dir, service, task)
}

#[tokio::test]
async fn publication_receipts_share_one_commit() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_dir, service, _) = planned_export_fixture().await;
    let project = service.store.projects().await.unwrap()[0].id.clone();
    for index in 0..16 {
        service
            .store
            .execute(
                json!({"command":"job.create","project":project,"title":format!("Extra {index}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let state = service.store.snapshot().await.unwrap();
    let commits = Arc::new(AtomicUsize::new(0));
    // Install on every connection without allowing the pool to reuse one slot.
    let mut connections = Vec::new();
    for _ in 0..4 {
        let mut conn = service.store.pool.acquire().await.unwrap();
        let measured = commits.clone();
        conn.lock_handle().await.unwrap().set_commit_hook(move || {
            measured.fetch_add(1, Ordering::Relaxed);
            true
        });
        connections.push(conn);
    }
    drop(connections);
    service
        .render_state_locked(&state, None, &BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(
        commits.load(Ordering::Relaxed),
        1,
        "Plan hashes, Job metadata, and document receipts must commit together"
    );
}

#[tokio::test]
async fn failed_document_receipt_rolls_back_plan_and_goal_metadata() {
    let (_dir, service, _) = planned_export_fixture().await;
    sqlx::query(
        "UPDATE plans SET data=json_set(data,'$.hash','old-hash','$.pending_body','Pending body')",
    )
    .execute(&service.store.pool)
    .await
    .unwrap();
    let state = service.store.snapshot().await.unwrap();
    let key = format!("goal:{}", state.jobs[0].id);
    service
        .store
        .set_metadata(&key, &json!("Previous goal"))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_receipt BEFORE INSERT ON document_registry BEGIN SELECT RAISE(ABORT,'receipt failure'); END").execute(&service.store.pool).await.unwrap();
    let error = service
        .render_state_locked(&state, None, &BTreeMap::new())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("receipt failure"));
    let after = service.store.snapshot().await.unwrap();
    assert_eq!(after.plans, state.plans);
    assert_eq!(
        service.store.metadata(&key).await.unwrap(),
        Some(json!("Previous goal"))
    );
    sqlx::query("DROP TRIGGER reject_receipt")
        .execute(&service.store.pool)
        .await
        .unwrap();
    service
        .render_state_locked(&after, None, &BTreeMap::new())
        .await
        .unwrap();
    let recovered = service.store.snapshot().await.unwrap();
    assert!(recovered.plans[0].pending_body.is_none());
    assert_ne!(recovered.plans[0].hash, "old-hash");
    assert_eq!(
        service.store.metadata(&key).await.unwrap(),
        Some(json!(state.jobs[0].goal))
    );
}

#[tokio::test]
async fn stale_plan_receipt_preserves_the_newer_version() {
    let (_dir, service, _) = planned_export_fixture().await;
    let state = service.store.snapshot().await.unwrap();
    let plan = &state.plans[0];
    sqlx::query("UPDATE plans SET data=json_set(data,'$.version',?,'$.hash','newer-hash','$.pending_body','Newer body') WHERE id=?")
        .bind(plan.version + 1).bind(&plan.id).execute(&service.store.pool).await.unwrap();
    let expected = service.store.snapshot().await.unwrap().plans;
    let metadata = crate::publication::PublicationMetadata {
        plans: vec![crate::publication::PublishedPlan {
            id: plan.id.clone(),
            version: plan.version,
            hash: "stale-hash".into(),
        }],
        ..Default::default()
    };
    service
        .store
        .acknowledge_documents(
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeSet::new(),
            0,
            &metadata,
        )
        .await
        .unwrap();
    assert_eq!(service.store.snapshot().await.unwrap().plans, expected);
}
