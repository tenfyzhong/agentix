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
async fn projection_keeps_complete_content_and_reason_in_body_only() {
    let (_dir, store, projection, id) = fixture().await;
    let mut content = input();
    content.conditions = vec!["Customer-managed: production".into()];
    content.valid_until = Some(4_000_000_000);
    let memory = store
        .supersede(
            "project",
            &id,
            1,
            content,
            "New external constraint",
            Actor::Human,
        )
        .await
        .unwrap();
    projection.sync("project", "", 20).await.unwrap();
    let text = std::fs::read_to_string(projection.path(&memory.id).unwrap()).unwrap();
    let (header, body) = text
        .strip_prefix("---\n")
        .unwrap()
        .split_once("---\n")
        .unwrap();
    let metadata: serde_json::Value = serde_yaml::from_str(header).unwrap();
    assert!(metadata.get("content").is_none());
    assert!(metadata.get("reason").is_none());
    assert_eq!(metadata["id"], memory.id);
    assert_eq!(metadata["project_id"], "project");
    assert_eq!(metadata["revision"], memory.revision);
    assert_eq!(metadata["status"], "active");
    assert!(body.contains("## content\n"));
    let fields = serde_json::to_value(&memory.content).unwrap();
    for (field, expected) in fields.as_object().unwrap() {
        let start = format!("<!-- taskix-memory:{field} -->\n");
        let end = format!("\n<!-- /taskix-memory:{field} -->");
        let text = body
            .split_once(&start)
            .unwrap()
            .1
            .split_once(&end)
            .unwrap()
            .0;
        if let Some(expected) = expected.as_str() {
            assert_eq!(text, expected, "{field}");
        } else {
            let yaml = text
                .lines()
                .map(|line| line.strip_prefix("    ").unwrap())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                serde_yaml::from_str::<serde_json::Value>(&yaml).unwrap(),
                *expected,
                "{field}"
            );
        }
    }
    assert!(body.contains("## reason\n"));
    assert!(body.contains(
        "<!-- taskix-memory:reason -->\nNew external constraint\n<!-- /taskix-memory:reason -->"
    ));
}

#[tokio::test]
async fn projection_rewrites_legacy_frontmatter_without_changing_database_revision() {
    let (_dir, store, projection, id) = fixture().await;
    projection.sync("project", "", 20).await.unwrap();
    let path = projection.path(&id).unwrap();
    let canonical = std::fs::read_to_string(&path).unwrap();
    let memory = store.show("project", &id, None).await.unwrap();
    let legacy = format!(
        "---\n{}---\n\nOld memory layout\n",
        serde_yaml::to_string(&memory).unwrap()
    );
    std::fs::write(&path, legacy).unwrap();
    let page = projection.sync("project", "", 20).await.unwrap();
    assert_eq!(page.published, 1);
    assert_eq!(page.imported, 0);
    assert!(page.conflicts.is_empty());
    let repaired = std::fs::read_to_string(path).unwrap();
    assert_eq!(repaired, canonical);
    let (header, _) = repaired
        .strip_prefix("---\n")
        .unwrap()
        .split_once("---\n")
        .unwrap();
    let metadata: serde_json::Value = serde_yaml::from_str(header).unwrap();
    assert!(metadata.get("content").is_none());
    assert!(metadata.get("reason").is_none());
    assert_eq!(store.show("project", &id, None).await.unwrap(), memory);
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

#[tokio::test]
async fn unchanged_projection_does_not_update_publication_receipt() {
    let (dir, _store, projection, _id) = fixture().await;
    projection.sync("project", "", 20).await.unwrap();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("memory.db")),
    )
    .await
    .unwrap();
    sqlx::raw_sql("CREATE TABLE receipt_updates(n INTEGER); CREATE TRIGGER count_receipt_updates AFTER UPDATE ON memory_projection BEGIN INSERT INTO receipt_updates VALUES(1); END;").execute(&pool).await.unwrap();
    let page = projection.sync("project", "", 20).await.unwrap();
    assert_eq!(page.published, 0);
    let updates: i64 = sqlx::query_scalar("SELECT count(*) FROM receipt_updates")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        updates, 0,
        "unchanged notes must not dirty the receipt table"
    );
}

#[tokio::test]
async fn pending_projection_prioritizes_changes_and_full_sync_repairs_external_edits() {
    let (_dir, store, projection, id) = fixture().await;
    assert_eq!(
        projection
            .sync_pending("project", "", 20)
            .await
            .unwrap()
            .published,
        1
    );
    let path = projection.path(&id).unwrap();
    std::fs::write(&path, "External edit").unwrap();
    assert_eq!(
        projection
            .sync_pending("project", "", 20)
            .await
            .unwrap()
            .published,
        0
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "External edit");
    projection.sync("project", "", 20).await.unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Use the regional endpoint")
    );
    let mut changed = input();
    changed.conclusion = "Updated constraint".into();
    store
        .update("project", &id, 1, changed, Actor::Human)
        .await
        .unwrap();
    assert_eq!(
        projection
            .sync_pending("project", "", 20)
            .await
            .unwrap()
            .published,
        1
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Updated constraint")
    );
    assert_eq!(
        projection
            .sync_pending("other", "", 20)
            .await
            .unwrap()
            .published,
        0
    );
}
