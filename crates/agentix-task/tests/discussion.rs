use agentix_task::{Store, WriteOptions};
use serde_json::{Value, json};

fn options() -> WriteOptions {
    WriteOptions {
        session_ref: Some("discussion-session".into()),
        actor_ref: "agent:codex".into(),
        ..WriteOptions::default()
    }
}

async fn record(store: &Store, turn: &str, prompt: &str) -> Value {
    store.execute(json!({"command":"session.record","session":"discussion-session",
        "turn_id":turn,"messages":[
            {"id":format!("{turn}:user"),"role":"user","text":prompt},
            {"id":format!("{turn}:assistant"),"role":"assistant","text":format!("Reply to {prompt}")}
        ]}), options()).await.unwrap().result
}

#[tokio::test]
async fn discussion_survives_without_job_and_attaches_selected_turns_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let result = record(&store, "one", "Why is the message truncated?").await;
    assert_eq!(
        result["staged"], true,
        "discussion must survive before a Job exists"
    );
    record(&store, "other", "Unrelated question").await;
    record(&store, "two", "Can we split cards?").await;
    record(&store, "three", "Keep short content in one card").await;
    record(&store, "implement", "Implement this").await;
    assert!(store.snapshot().await.unwrap().jobs.is_empty());
    drop(store);
    let store = Store::open(&path).await.unwrap();
    let project = store
        .execute(
            json!({"command":"project.register","name":"Test","root":dir.path()}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let job = store.execute(json!({"command":"job.create","project":project["id"],"title":"Cards","prompt":"Implement this",
        "conversation_turns":["implement","three","one","two"]}), options()).await.unwrap().result;
    assert_eq!(job["prompt"], "Why is the message truncated?");
    let texts: Vec<_> = job["conversation"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["text"].clone())
        .collect();
    assert_eq!(
        texts,
        json!([
            "Why is the message truncated?",
            "Can we split cards?",
            "Keep short content in one card",
            "Implement this"
        ])
        .as_array()
        .unwrap()
        .clone()
    );
    record(&store, "implement", "Implement this").await;
    let state = store.snapshot().await.unwrap();
    assert_eq!(state.jobs[0].conversation.len(), 8);
    record(&store, "later", "Another unrelated discussion").await;
    assert_eq!(
        store.snapshot().await.unwrap().jobs[0].conversation.len(),
        8
    );
}

#[tokio::test]
async fn selection_revision_conflict_rolls_back_job_creation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    record(&store, "a", "Original").await;
    let snapshot = store
        .discussion_list("discussion-session", 0, 100)
        .await
        .unwrap();
    record(&store, "b", "Changed candidates").await;
    let project = store
        .execute(
            json!({"command":"project.register","name":"Test","root":dir.path()}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let error = store.execute(json!({"command":"job.create","project":project["id"],"title":"Change","conversation_turns":["a"],"conversation_revision":snapshot["revision"]}),options()).await.unwrap_err();
    assert!(error.to_string().contains("candidates changed"));
    assert!(store.snapshot().await.unwrap().jobs.is_empty());
    assert_eq!(
        store
            .discussion_list("discussion-session", 0, 100)
            .await
            .unwrap()["turns"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn unassigned_drafts_expire_after_session_inactivity_but_bound_turns_survive() {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicI64::new(1_790_000_000));
    let now = clock.clone();
    let store = Store::open_with_clock(
        &dir.path().join("tasks.sqlite3"),
        Arc::new(move || now.load(Ordering::SeqCst)),
    )
    .await
    .unwrap();
    record(&store, "a", "Original").await;
    let project = store
        .execute(
            json!({"command":"project.register","name":"Test","root":dir.path()}),
            options(),
        )
        .await
        .unwrap()
        .result;
    store.execute(json!({"command":"job.create","project":project["id"],"title":"Change","conversation_turns":["a"]}),options()).await.unwrap();
    record(&store, "b", "Draft").await;
    clock.fetch_add(29 * 86400, Ordering::SeqCst);
    assert_eq!(
        store
            .discussion_list("discussion-session", 0, 100)
            .await
            .unwrap()["turns"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    clock.fetch_add(86400, Ordering::SeqCst);
    store
        .execute(
            json!({"command":"session.start","session":"discussion-session"}),
            options(),
        )
        .await
        .unwrap();
    assert!(
        store
            .discussion_show("discussion-session", "b")
            .await
            .is_err()
    );
    assert!(
        store
            .discussion_show("discussion-session", "a")
            .await
            .is_ok()
    );
    assert_eq!(
        store.snapshot().await.unwrap().jobs[0].conversation.len(),
        2
    );
}

#[tokio::test]
async fn selected_turn_cannot_be_stolen_and_late_replies_keep_original_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    record(&store, "a", "Original").await;
    record(&store, "b", "Implement").await;
    let project = store
        .execute(
            json!({"command":"project.register","name":"Test","root":dir.path()}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let create = json!({"command":"job.create","project":project["id"],"title":"Change","conversation_turns":["a","b"]});
    store.execute(create.clone(), options()).await.unwrap();
    assert!(
        store
            .execute(create, options())
            .await
            .unwrap_err()
            .to_string()
            .contains("another Job")
    );
    store.execute(json!({"command":"session.record","session":"discussion-session","turn_id":"a","messages":[{"id":"late","role":"assistant","text":"Late original reply"}]}),options()).await.unwrap();
    let state = store.snapshot().await.unwrap();
    assert_eq!(state.jobs.len(), 1);
    assert_eq!(state.jobs[0].conversation[2].text, "Late original reply");
    assert_eq!(state.jobs[0].conversation[3].text, "Implement");
}

#[tokio::test]
async fn followup_preserves_prompt_and_places_discussion_before_implementation() {
    use sqlx::Connection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let project = store
        .execute(
            json!({"command":"project.register","name":"Test","root":dir.path()}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let job=store.execute(json!({"command":"job.create","project":project["id"],"title":"Original","prompt":"Original requirement"}),options()).await.unwrap().result;
    let mut conn = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET data=json_set(data,'$.status','PENDING_REVIEW') WHERE id=?")
        .bind(job["id"].as_str().unwrap())
        .execute(&mut conn)
        .await
        .unwrap();
    record(&store, "a", "Should we also split code blocks?").await;
    record(&store, "b", "Implement the supplement").await;
    let followed=store.execute(json!({"command":"job.followup","job":job["id"],"prompt":"Implement the supplement","conversation_turns":["a","b"]}),options()).await.unwrap().result;
    assert_eq!(followed["prompt"], "Original requirement");
    assert_eq!(
        followed["conversation"][0]["text"],
        "Should we also split code blocks?"
    );
    assert_eq!(
        followed["conversation"][2]["text"],
        "Implement the supplement"
    );
    assert_eq!(followed["conversation"].as_array().unwrap().len(), 4);
    assert_eq!(followed["status"], "ACTIVE");
}

#[tokio::test]
async fn discussion_filters_path_qualified_host_context_before_storage() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    record(&store,"context","# AGENTS.md instructions for /work\n<INSTRUCTIONS>hidden rules</INSTRUCTIONS><environment_context>hidden environment</environment_context>\nActual request").await;
    let draft = store
        .discussion_show("discussion-session", "context")
        .await
        .unwrap();
    assert_eq!(draft["messages"][0]["text"], "Actual request");
}

#[tokio::test]
async fn changed_delivery_target_rejects_stale_discussion_selection() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    record(&store, "a", "Discuss cards").await;
    let project = store
        .execute(
            json!({"command":"project.register","name":"Test","root":dir.path()}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let result=store.execute(json!({"command":"job.create","project":project["id"],"title":"Different delivery","prompt":"Implement","conversation_turns":["a"],"conversation_target":"stale target"}),options()).await;
    assert!(
        result.is_err(),
        "changing the classified delivery must invalidate selection"
    );
    assert!(store.snapshot().await.unwrap().jobs.is_empty());
}

#[tokio::test]
async fn draft_capture_does_not_load_a_previous_jobs_body() {
    use agentix_task::{Config, DocumentConfig, Service, StorageConfig};
    use sqlx::Connection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let service = Service::open(Config {
        schema_version: 1,
        storage: StorageConfig { path: path.clone() },
        documents: DocumentConfig {
            root: dir.path().into(),
            directory: "notes".into(),
        },
    })
    .await
    .unwrap();
    let project = service
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Scope"}),
            options(),
        )
        .await
        .unwrap()
        .result;
    service
        .execute(
            json!({"command":"job.create","project":project["id"],"title":"Previous"}),
            options(),
        )
        .await
        .unwrap();
    let mut conn = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await
    .unwrap();
    let lock = service.config().output_dir().join(".taskix.lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    // A previous Job must not even be deserialized when recording an unbound turn.
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title')")
        .execute(&mut conn)
        .await
        .unwrap();
    let captured = service.execute(json!({"command":"session.record","session":"discussion-session","turn_id":"new","messages":[{"id":"new:user","role":"user","text":"Independent discussion"}]}),options()).await.unwrap();
    assert_eq!(captured.result["staged"], true);
    assert!(
        captured.projection_pending.is_none(),
        "drafts need no Obsidian lock"
    );
    assert_eq!(
        service
            .store()
            .discussion_list("discussion-session", 0, 100)
            .await
            .unwrap()["turns"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn bound_capture_does_not_reparse_earlier_draft_bodies() {
    use sqlx::Connection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let store = Store::open(&path).await.unwrap();
    record(&store, "old", "Original").await;
    record(&store, "now", "Implement").await;
    let project = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Scope"}),
            options(),
        )
        .await
        .unwrap()
        .result;
    store.execute(json!({"command":"job.create","project":project["id"],"title":"Scoped capture","conversation_turns":["old","now"]}),options()).await.unwrap();
    let mut conn = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE discussion_turns SET messages='{}' WHERE turn_id='old'")
        .execute(&mut conn)
        .await
        .unwrap();
    let capture = json!({"command":"session.record","session":"discussion-session","turn_id":"now","messages":[{"id":"final","role":"assistant","text":"Final response"}]});
    store.execute(capture.clone(), options()).await.unwrap();
    store.execute(capture, options()).await.unwrap();
    let job = &store.snapshot().await.unwrap().jobs[0];
    assert_eq!(job.conversation.len(), 5);
    assert_eq!(job.conversation.last().unwrap().text, "Final response");
}

#[tokio::test]
async fn malformed_selection_guards_never_silently_create_an_unbound_job() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    record(&store, "one", "Original").await;
    let project = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Validation"}),
            options(),
        )
        .await
        .unwrap()
        .result;
    for patch in [
        json!({"conversation_turns":"one"}),
        json!({"conversation_turns":[],"conversation_revision":1}),
        json!({"conversation_turns":["one"],"conversation_revision":"1"}),
        json!({"conversation_turns":["one"],"conversation_target":42}),
    ] {
        let mut request = json!({"command":"job.create","project":project["id"],"title":"Guarded"});
        request
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(
            store.execute(request, options()).await.is_err(),
            "invalid selection must reject atomically: {patch}"
        );
        assert!(store.snapshot().await.unwrap().jobs.is_empty());
    }
}

#[tokio::test]
async fn deleting_a_job_removes_its_bound_source_copies_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    record(&store, "bound", "Delete this history").await;
    record(&store, "pending", "Keep this draft").await;
    let project = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Delete"}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let job=store.execute(json!({"command":"job.create","project":project["id"],"title":"Delete","conversation_turns":["bound"]}),options()).await.unwrap().result;
    store
        .execute(json!({"command":"job.cancel","job":job["id"]}), options())
        .await
        .unwrap();
    store
        .execute(json!({"command":"job.delete","job":job["id"]}), options())
        .await
        .unwrap();
    assert!(
        store
            .discussion_show("discussion-session", "bound")
            .await
            .is_err()
    );
    assert!(
        store
            .discussion_show("discussion-session", "pending")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn concurrent_selection_commits_exactly_one_job() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    record(&store, "one", "Original").await;
    let project = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Concurrent"}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let revision = store
        .discussion_index("discussion-session", 0, 1)
        .await
        .unwrap()["revision"]
        .clone();
    let request = json!({"command":"job.create","project":project["id"],"title":"Selected","conversation_turns":["one"],"conversation_revision":revision});
    let (first, second) = tokio::join!(
        store.execute(request.clone(), options()),
        store.execute(request, options())
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(store.snapshot().await.unwrap().jobs.len(), 1);
}

#[tokio::test]
async fn unchanged_bound_capture_skips_job_loading_and_projection() {
    use sqlx::Connection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let store = Store::open(&path).await.unwrap();
    record(&store, "one", "Original").await;
    let project = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Replay"}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let job=store.execute(json!({"command":"job.create","project":project["id"],"title":"Replay","conversation_turns":["one"]}),options()).await.unwrap().result;
    let mut conn = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title')")
        .execute(&mut conn)
        .await
        .unwrap();
    let outcome = record(&store, "one", "Original").await;
    assert_eq!(outcome["job_id"], job["id"]);
    assert_eq!(outcome["recorded"], 0);
}

#[tokio::test]
#[ignore = "manual discussion batch benchmark"]
async fn discussion_batch_scaling_benchmark() {
    for count in [1000, 4000] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let messages:Vec<_>=(0..count).map(|i|json!({"id":format!("m{i}"),"role":if i%2==0 {"user"} else {"assistant"},"text":format!("Original message {i}")})).collect();
        let request = json!({"command":"session.record","session":"discussion-session","turn_id":"batch","messages":messages});
        let began = std::time::Instant::now();
        store.execute(request.clone(), options()).await.unwrap();
        let stage = began.elapsed();
        let project = store
            .execute(
                json!({"command":"project.register","root":dir.path(),"name":"Benchmark"}),
                options(),
            )
            .await
            .unwrap()
            .result;
        let began = std::time::Instant::now();
        let job=store.execute(json!({"command":"job.create","project":project["id"],"title":"Batch","conversation_turns":["batch"]}),options()).await.unwrap().result;
        let attach = began.elapsed();
        let began = std::time::Instant::now();
        store.execute(request, options()).await.unwrap();
        println!(
            "messages={count} stage={stage:?} attach={attach:?} replay={:?}",
            began.elapsed()
        );
        assert_eq!(job["conversation"].as_array().unwrap().len(), count);
    }
}
