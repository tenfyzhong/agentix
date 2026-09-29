use agentix_memory::{MemoryStore, ProjectTools, Source, ToolSet};
use serde_json::json;

#[tokio::test]
async fn source_discovery_pages_turns_and_message_lists() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    for sequence in 1..=12 {
        let source: Source = serde_json::from_value(json!({"instance_id":"db","receipt_id":format!("r{sequence}"),"sequence":sequence,"project_id":"p","session_id":"s","turn_id":format!("legacy:job:{sequence}"),"revision":1,"job_id":null,"recorded_at":sequence,"messages":(0..40).map(|n|json!({"id":format!("m{n}"),"role":"user","text":"Choice"})).collect::<Vec<_>>()})).unwrap();
        store.ingest(&source).await.unwrap();
    }
    let tools = ProjectTools::new(store, "p".into(), None).unwrap();
    let first = tools
        .execute("source_neighbors", json!({"receipt_id":"r12"}))
        .await
        .unwrap();
    assert_eq!(first["sources"].as_array().unwrap().len(), 8);
    let second = tools
        .execute(
            "source_neighbors",
            json!({"receipt_id":first["next_receipt_id"]}),
        )
        .await
        .unwrap();
    assert_eq!(second["sources"].as_array().unwrap().len(), 3);
    assert!(second["next_receipt_id"].is_null());
    let first = tools
        .execute(
            "source_read",
            json!({"receipt_id":"r1","message_id":null,"offset":0}),
        )
        .await
        .unwrap();
    assert_eq!(first["messages"].as_array().unwrap().len(), 32);
    let second = tools
        .execute(
            "source_read",
            json!({"receipt_id":"r1","message_id":null,"offset":first["next_offset"]}),
        )
        .await
        .unwrap();
    assert_eq!(second["messages"].as_array().unwrap().len(), 8);
    assert!(second["next_offset"].is_null());
}

#[tokio::test]
async fn source_neighbors_discover_prior_turns_without_crossing_project_or_session() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    for (sequence, receipt, project, session, turn, revision) in [
        (1, "old", "p", "s", "proposal", 1),
        (2, "proposal", "p", "s", "proposal", 2),
        (3, "foreign-project", "other", "s", "foreign", 1),
        (4, "foreign-session", "p", "other", "foreign", 1),
        (5, "choice", "p", "s", "choice", 1),
        (6, "future", "p", "s", "future", 1),
        (7, "updated-proposal", "p", "s", "proposal", 3),
    ] {
        let source: Source = serde_json::from_value(json!({"instance_id":"db","receipt_id":receipt,"sequence":sequence,"project_id":project,"session_id":session,"turn_id":turn,"revision":revision,"job_id":null,"recorded_at":sequence,"messages":[{"id":"message","role":"assistant","text":"Option B preserves offline access."}]})).unwrap();
        store.ingest(&source).await.unwrap();
    }
    let tools = ProjectTools::new(store, "p".into(), None).unwrap();
    let neighbors = tools
        .execute("source_neighbors", json!({"receipt_id":"choice"}))
        .await
        .unwrap();
    assert_eq!(neighbors["sources"].as_array().unwrap().len(), 1);
    assert_eq!(neighbors["sources"][0]["receipt_id"], "updated-proposal");
    let messages = tools
        .execute(
            "source_read",
            json!({"receipt_id":"proposal","message_id":null,"offset":0}),
        )
        .await
        .unwrap();
    assert_eq!(messages["messages"][0]["id"], "message");
    let text = tools
        .execute(
            "source_read",
            json!({"receipt_id":"proposal","message_id":"message","offset":0}),
        )
        .await
        .unwrap();
    assert!(text["page"]["text"].as_str().unwrap().contains("offline"));
    assert!(
        tools
            .execute("source_neighbors", json!({"receipt_id":"foreign-project"}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn tools_are_project_scoped_and_repository_reads_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "The decision is documented here.").unwrap();
    std::fs::write(repo.join("large.txt"), "界".repeat(10000)).unwrap();
    std::fs::write(temp.path().join("outside.txt"), "private").unwrap();
    std::fs::write(repo.join(".env"), "SECRET=private").unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let source: Source = serde_json::from_value(json!({"instance_id":"instance","receipt_id":"receipt","sequence":1,"project_id":"other","session_id":"session","turn_id":"turn","revision":1,"job_id":null,"recorded_at":1,"messages":[{"id":"message","role":"user","text":"private"}]})).unwrap();
    store.ingest(&source).await.unwrap();
    let tools = ProjectTools::new(store, "project".into(), Some(repo.clone())).unwrap();
    let read = tools
        .execute("repo_read", json!({"path":"README.md","offset":0}))
        .await
        .unwrap();
    assert!(read["text"].as_str().unwrap().contains("documented"));
    let large = tools
        .execute("repo_read", json!({"path":"large.txt","offset":0}))
        .await
        .unwrap();
    assert!(large["text"].as_str().unwrap().len() <= 8192);
    assert!(large["next_offset"].as_u64().unwrap() > 0);
    for path in ["../outside.txt", ".env"] {
        assert!(
            tools
                .execute("repo_read", json!({"path":path,"offset":0}))
                .await
                .is_err()
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path().join("outside.txt"), repo.join("link.txt")).unwrap();
        assert!(
            tools
                .execute("repo_read", json!({"path":"link.txt","offset":0}))
                .await
                .is_err()
        );
    }
    assert!(
        tools
            .execute(
                "source_read",
                json!({"receipt_id":"receipt","message_id":"message","offset":0})
            )
            .await
            .is_err()
    );
    assert!(
        tools
            .execute(
                "memory_search",
                json!({"query":"private","project_id":"other"})
            )
            .await
            .is_err()
    );
    let found = tools
        .execute("repo_search", json!({"query":"documented"}))
        .await
        .unwrap();
    assert_eq!(found["matches"][0]["path"], "README.md");
    assert!(tools.repository_checked());
}
