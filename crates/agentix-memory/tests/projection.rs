use agentix_memory::{Actor, Kind, MemoryInput, MemoryProjection, MemoryStore, Status};
use std::path::Path;

fn input() -> MemoryInput {
    MemoryInput {
        title: "Regional service".into(),
        conclusion: "Use the regional endpoint".into(),
        rationale: "External residency requirement".into(),
        scope: "Production".into(),
        conditions: vec![],
        valid_until: None,
        tags: vec!["region".into()],
        kind: Kind::UserAssertion,
        evidence: vec![],
    }
}
async fn fixture() -> (tempfile::TempDir, MemoryStore, MemoryProjection, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let memory = store
        .create("project", input(), Actor::Human)
        .await
        .unwrap();
    let projection = MemoryProjection::new(
        store.clone(),
        dir.path(),
        Path::new("Agents/Projects/demo/Memory"),
    )
    .unwrap();
    (dir, store, projection, memory.id)
}
#[tokio::test]
async fn projection_restores_all_file_edits_without_changing_memory() {
    let (_dir, store, projection, id) = fixture().await;
    projection.sync("project", "", 20).await.unwrap();
    let path = projection.path(&id).unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    for edited in [
        original.replace("Use the regional endpoint", "Use the European endpoint"),
        original.replace("project_id: project", "project_id: foreign"),
        original.replace("user_assertion", "inference"),
        format!("{original}\nExtra human notes\n"),
        "invalid YAML and arbitrary content".into(),
    ] {
        std::fs::write(&path, &edited).unwrap();
        let page = projection.sync("project", "", 20).await.unwrap();
        assert_eq!(page.imported, 0);
        assert!(page.conflicts.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let memory = store.show("project", &id, None).await.unwrap();
        assert_eq!(memory.revision, 1);
        assert_eq!(memory.content, input());
    }
    assert!(original.contains("Read-only"));
    std::fs::remove_file(&path).unwrap();
    projection.sync("project", "", 20).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert_eq!(
        store.show("project", &id, None).await.unwrap().status,
        Status::Active
    );
}
#[tokio::test]
async fn database_update_wins_over_stale_note_edits() {
    let (_dir, store, projection, id) = fixture().await;
    projection.sync("project", "", 20).await.unwrap();
    let path = projection.path(&id).unwrap();
    std::fs::write(&path, "Unaccepted human edit").unwrap();
    let mut updated = input();
    updated.conclusion = "Concurrent database edit".into();
    store
        .update("project", &id, 1, updated.clone(), Actor::Human)
        .await
        .unwrap();
    let page = projection.sync("project", "", 20).await.unwrap();
    assert_eq!(page.imported, 0);
    assert!(page.conflicts.is_empty());
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains(&updated.conclusion)
    );
    let memory = store.show("project", &id, None).await.unwrap();
    assert_eq!(memory.revision, 2);
    assert_eq!(memory.content, updated);
}
#[tokio::test]
async fn failed_publication_remains_pending_across_store_reopen() {
    let (dir, store, projection, id) = fixture().await;
    let path = projection.path(&id).unwrap();
    std::fs::write(dir.path().join("Agents"), "blocked directory").unwrap();
    assert_eq!(
        projection
            .sync("project", "", 20)
            .await
            .unwrap()
            .conflicts
            .len(),
        1
    );
    assert_eq!(
        store.projection_status("project").await.unwrap()["pending"],
        1
    );
    std::fs::remove_file(dir.path().join("Agents")).unwrap();
    let reopened = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let projection = MemoryProjection::new(
        reopened.clone(),
        dir.path(),
        Path::new("Agents/Projects/demo/Memory"),
    )
    .unwrap();
    assert_eq!(
        projection.sync("project", "", 20).await.unwrap().published,
        1
    );
    assert!(path.exists());
    assert_eq!(
        reopened.projection_status("project").await.unwrap()["pending"],
        0
    );
}
#[cfg(unix)]
#[tokio::test]
async fn projection_rejects_symlink_escape_and_keeps_foreign_files_untouched() {
    let (dir, _store, projection, _id) = fixture().await;
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("Agents")).unwrap();
    assert_eq!(
        projection
            .sync("project", "", 20)
            .await
            .unwrap()
            .conflicts
            .len(),
        1
    );
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    assert!(
        MemoryProjection::new(
            MemoryStore::open(&dir.path().join("other.db"))
                .await
                .unwrap(),
            dir.path(),
            Path::new("../escape")
        )
        .is_err()
    );
}

#[tokio::test]
async fn failed_repair_never_imports_edits_and_retries() {
    let (_dir, store, projection, id) = fixture().await;
    projection.sync("project", "", 20).await.unwrap();
    let path = projection.path(&id).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace("Use the regional endpoint", "A human clarification"),
    )
    .unwrap();
    let recovery = path.parent().unwrap().join("Recovery");
    std::fs::write(&recovery, "publication failure").unwrap();
    assert_eq!(
        projection
            .sync("project", "", 20)
            .await
            .unwrap()
            .conflicts
            .len(),
        1
    );
    assert_eq!(store.show("project", &id, None).await.unwrap().revision, 1);
    std::fs::remove_file(recovery).unwrap();
    projection.sync("project", "", 20).await.unwrap();
    assert_eq!(store.show("project", &id, None).await.unwrap().revision, 1);
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains("Use the regional endpoint")
    );
}
#[tokio::test]
async fn prepared_receipt_recovers_a_file_installed_before_database_acknowledgement() {
    use sqlx::Connection;
    let (dir, store, projection, id) = fixture().await;
    let mut connection = sqlx::SqliteConnection::connect(&format!(
        "sqlite://{}",
        dir.path().join("memory.db").display()
    ))
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_publication_ack BEFORE UPDATE OF published_revision ON memory_projection BEGIN SELECT RAISE(FAIL,'simulated acknowledgement failure'); END").execute(&mut connection).await.unwrap();
    assert_eq!(
        projection
            .sync("project", "", 20)
            .await
            .unwrap()
            .conflicts
            .len(),
        1
    );
    assert!(projection.path(&id).unwrap().exists());
    sqlx::query("DROP TRIGGER fail_publication_ack")
        .execute(&mut connection)
        .await
        .unwrap();
    let reopened = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let projection = MemoryProjection::new(
        reopened,
        dir.path(),
        Path::new("Agents/Projects/demo/Memory"),
    )
    .unwrap();
    let result = projection.sync("project", "", 20).await.unwrap();
    assert!(result.conflicts.is_empty());
    assert_eq!(result.imported, 0);
    assert_eq!(store.show("project", &id, None).await.unwrap().revision, 1);
    assert_eq!(
        store.projection_status("project").await.unwrap()["pending"],
        0
    );
}
