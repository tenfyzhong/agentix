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
