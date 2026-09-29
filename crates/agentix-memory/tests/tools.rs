use agentix_memory::{MemoryStore, ProjectTools, Source, ToolSet};
use serde_json::json;

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
