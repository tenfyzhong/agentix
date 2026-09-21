use agentix_task::{Store, WriteOptions};
use serde_json::json;

async fn fixture() -> (tempfile::TempDir, Store, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let project = store
        .execute(
            json!({"command":"project.register","id":"project","name":"Routing","root":dir.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (dir, store, project)
}

#[tokio::test]
async fn routing_snapshot_empty_project_is_complete() {
    let (_dir, store, project) = fixture().await;
    let result = store.routing_candidates(&project).await.unwrap();
    assert_eq!(result["candidates"], json!([]));
    assert_eq!(result["complete"], true);
}

#[tokio::test]
async fn routing_snapshot_bounds_jobs_and_marks_overflow() {
    let (_dir, store, project) = fixture().await;
    for i in 0..33 {
        store
            .execute(
                json!({"command":"job.create","project":project,"title":format!("Job {i}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let result = store.routing_candidates(&project).await.unwrap();
    assert_eq!(result["candidates"].as_array().unwrap().len(), 32);
    assert_eq!(result["complete"], false);
}

#[tokio::test]
async fn routing_snapshot_bounds_text_and_preserves_waiting_reason() {
    let (_dir, store, project) = fixture().await;
    let job = store.execute(json!({"command":"job.create","project":project,"title":"Work","prompt":"界".repeat(100_000)}), WriteOptions::default()).await.unwrap().result;
    let task = store
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Wait"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    store
        .execute(
            json!({"command":"task.wait","task":task["id"],"reason":"Which region?"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let result = store.routing_candidates(&project).await.unwrap();
    assert_eq!(result["complete"], false);
    assert!(result.to_string().len() < 20_000);
    assert_eq!(
        result["candidates"][0]["tasks"][0]["reason"],
        "Which region?"
    );
    assert_eq!(
        result["candidates"][0]["tasks"][0]["status"],
        "WAITING_USER"
    );
}

#[tokio::test]
async fn routing_snapshot_scopes_jobs_and_retains_recent_conversation() {
    let (_dir, store, project) = fixture().await;
    let job = store
        .execute(
            json!({"command":"job.create","project":project,"title":"Work"}),
            WriteOptions {
                session_ref: Some("session".into()),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap()
        .result;
    let cancelled = store
        .execute(
            json!({"command":"job.create","project":project,"title":"Cancelled"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    store
        .execute(
            json!({"command":"job.cancel","job":cancelled["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let messages: Vec<_> = (0..10)
        .map(|i| json!({"id":format!("m{i}"),"role":"user","text":format!("Message {i}")}))
        .collect();
    store.execute(json!({"command":"session.record","session":"session","job":job["id"],"messages":messages}), WriteOptions { session_ref: Some("session".into()), ..WriteOptions::default() }).await.unwrap();
    let result = store.routing_candidates(&project).await.unwrap();
    assert_eq!(result["complete"], true);
    assert_eq!(result["candidates"].as_array().unwrap().len(), 1);
    let conversation = result["candidates"][0]["job"]["conversation"]
        .as_array()
        .unwrap();
    assert_eq!(conversation.len(), 6);
    assert_eq!(conversation[0]["text"], "Message 4");
    assert_eq!(conversation[5]["text"], "Message 9");
    assert_eq!(
        store.routing_candidates("unrelated").await.unwrap()["candidates"],
        json!([])
    );
}

#[tokio::test]
async fn routing_snapshot_marks_task_overflow_instead_of_hiding_candidates() {
    let (_dir, store, project) = fixture().await;
    let job = store
        .execute(
            json!({"command":"job.create","project":project,"title":"Work"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    for i in 0..257 {
        store
            .execute(
                json!({"command":"task.add","job":job["id"],"title":format!("Task {i}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let result = store.routing_candidates(&project).await.unwrap();
    assert_eq!(result["complete"], false);
    assert_eq!(
        result["candidates"][0]["tasks"].as_array().unwrap().len(),
        256
    );
}

#[tokio::test]
async fn routing_revision_reads_only_identity_and_lifecycle() {
    let (_dir, store, project) = fixture().await;
    let job = store.execute(json!({"command":"job.create","project":project,"title":"Work","prompt":"secret context"}), WriteOptions::default()).await.unwrap().result;
    let id = job["id"].as_str().unwrap();
    let result = store.routing_revision(id).await.unwrap();
    assert_eq!(
        result,
        json!({"id":id,"project_id":project,"revision":job["revision"],"status":"ACTIVE","archived_at":null})
    );
    assert_eq!(
        store.routing_revision("missing").await.unwrap(),
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn routing_assignment_returns_references_without_task_bodies_or_leases() {
    let (_dir, store, project) = fixture().await;
    let job = store
        .execute(
            json!({"command":"job.create","project":project,"title":"Work"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let task = store
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Private task body"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    store.execute(json!({"command":"task.claim","task":task["id"],"executor":"agent:codex","session":"routing"}), WriteOptions::default()).await.unwrap();
    let result = store.routing_assignment("routing").await.unwrap();
    assert_eq!(
        result,
        json!({"project_id":project,"job_id":job["id"],"task_id":task["id"],"inbox_id":null})
    );
    assert_eq!(
        store.routing_assignment("other").await.unwrap()["job_id"],
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn routing_inbox_bounds_content_and_marks_incomplete() {
    let (_dir, store, project) = fixture().await;
    store
        .execute(
            json!({"command":"inbox.add","project":project,"content":"界".repeat(10000)}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let result = store.routing_inbox(&project).await.unwrap();
    assert_eq!(result["complete"], false);
    assert_eq!(result["entries"].as_array().unwrap().len(), 1);
    assert!(result.to_string().len() < 7000);
}

#[tokio::test]
async fn history_excerpts_do_not_mark_candidate_identity_incomplete() {
    let (_dir, store, project) = fixture().await;
    let job = store
        .execute(
            json!({"command":"job.create","project":project,"title":"Work"}),
            WriteOptions {
                session_ref: Some("s".into()),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap()
        .result;
    store.execute(json!({"command":"session.record","session":"s","job":job["id"],"messages":[{"id":"long","role":"assistant","text":"x".repeat(5000)}]}), WriteOptions { session_ref: Some("s".into()), ..WriteOptions::default() }).await.unwrap();
    let result = store.routing_candidates(&project).await.unwrap();
    assert_eq!(result["complete"], true);
    let message = &result["candidates"][0]["job"]["conversation"][0];
    assert_eq!(message["excerpt"], true);
    assert_eq!(message["text"].as_str().unwrap().len(), 2000);
}

#[tokio::test]
async fn terminal_tasks_do_not_consume_unfinished_candidate_budget() {
    let (_dir, store, project) = fixture().await;
    let job = store
        .execute(
            json!({"command":"job.create","project":project,"title":"Work"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    for i in 0..257 {
        let task = store
            .execute(
                json!({"command":"task.add","job":job["id"],"title":format!("Old {i}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        store
            .execute(
                json!({"command":"task.cancel","task":task["id"]}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let waiting = store
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Current"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let result = store.routing_candidates(&project).await.unwrap();
    assert_eq!(result["complete"], true);
    assert_eq!(
        result["candidates"][0]["tasks"].as_array().unwrap().len(),
        1
    );
    assert_eq!(result["candidates"][0]["tasks"][0]["id"], waiting["id"]);
}
