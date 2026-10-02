use agentix_memory::{Actor, Kind, Memory, MemoryInput, MemoryProjection, MemoryStore, Status};
use serde_json::json;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use std::path::Path;

const OLD: &str = "mem_01900000000070008000000000000001";
const NEXT: &str = "mem_01900000000070008000000000000002";
const CREATED: i64 = 1_790_956_800;

fn input() -> MemoryInput {
    MemoryInput {
        title: "External residency decision".into(),
        conclusion: format!("Original quote mentions {OLD}"),
        rationale: "Regional constraint".into(),
        scope: "Production".into(),
        conditions: vec![],
        valid_until: None,
        tags: vec!["residency".into()],
        kind: Kind::UserAssertion,
        evidence: vec![],
    }
}
fn local_id(old: &str, timestamp: i64) -> String {
    let instant = time::OffsetDateTime::from_unix_timestamp(timestamp).unwrap();
    let local = instant.to_offset(time::UtcOffset::local_offset_at(instant).unwrap());
    format!(
        "mem_{:02}{:02}{:02}{:02}{:02}{:02}_{}",
        local.year() % 100,
        u8::from(local.month()),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
        old.strip_prefix("mem_").unwrap()
    )
}
#[allow(clippy::too_many_lines)] // Seed a complete legacy database with all dependent tables.
async fn legacy(path: &Path) -> SqlitePool {
    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../src/schema.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version=1")
        .execute(&pool)
        .await
        .unwrap();
    for (id, status, supersedes, superseded_by) in [
        (OLD, Status::Superseded, None, Some(NEXT.into())),
        (NEXT, Status::Active, Some(OLD.into()), None),
    ] {
        let memory = Memory {
            id: id.into(),
            project_id: "p".into(),
            revision: 1,
            status,
            actor: Actor::Human,
            created_at: CREATED,
            updated_at: CREATED + 1,
            reason: "Original reason".into(),
            supersedes,
            superseded_by,
            content: input(),
        };
        let data = serde_json::to_string(&memory).unwrap();
        sqlx::query("INSERT INTO memories(id,project_id,revision,status,data) VALUES(?,'p',1,?,?)")
            .bind(id)
            .bind(serde_json::to_value(status).unwrap().as_str().unwrap())
            .bind(&data)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memory_versions VALUES(?,1,?)")
            .bind(id)
            .bind(data)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memory_projection(memory_id,published_revision) VALUES(?,1)")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO memory_vectors VALUES(?,'p',1,1,?)")
        .bind(NEXT)
        .bind(vec![0_u8; 8])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO embedding_failures VALUES(?,2,1,1,0,'retry')")
        .bind(NEXT)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO suppressions VALUES('p','evidence',?)")
        .bind(OLD)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO context_deliveries VALUES('p','s',?,1,0)")
        .bind(NEXT)
        .execute(&pool)
        .await
        .unwrap();
    let packet = json!({"text":format!("Historical header\n{}\n", json!({"id":NEXT,"conclusion":OLD})),"items":[{"id":NEXT,"revision":1}],"omitted":0});
    sqlx::query("INSERT INTO context_receipts VALUES('p','s','t',?,0)")
        .bind(packet.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let row: i64 = sqlx::query_scalar("SELECT rowid FROM memories WHERE id=?")
        .bind(NEXT)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO memory_fts(rowid,project_token,title,body,tags,scope) VALUES(?,'p','residency','','','')").bind(row).execute(&pool).await.unwrap();
    let source = json!({"instance_id":"db","receipt_id":"r","sequence":1,"project_id":"p",
        "session_id":"s","turn_id":"t","revision":1,"job_id":null,"recorded_at":CREATED,
        "messages":[{"id":"m","role":"user","text":OLD}]});
    sqlx::query("INSERT INTO sources VALUES('r','p','db',?)")
        .bind(source.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let review: serde_json::Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>("SELECT data FROM memories WHERE id=?")
            .bind(NEXT)
            .fetch_one(&pool)
            .await
            .unwrap(),
    )
    .unwrap();
    sqlx::query("INSERT INTO work_items(project_id,receipt_id,kind,state,payload,generation,owner,lease_until,result,attempts,max_attempts) VALUES('p','r','consolidate','running',?,3,'worker',100,?,1,1)")
        .bind(json!({"review":review,"fingerprint":"repo"}).to_string())
        .bind(json!([{"target":NEXT,"reason":OLD}]).to_string()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO memory_reviews VALUES(?,1,'repo',1)")
        .bind(NEXT)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO work_audits VALUES(1,3,?)")
        .bind(json!({"transcript":OLD}).to_string())
        .execute(&pool)
        .await
        .unwrap();
    pool
}

#[tokio::test]
async fn created_and_superseding_ids_use_local_creation_time_and_unique_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let first = store.create("p", input(), Actor::Human).await.unwrap();
    let suffix = first.id.rsplit('_').next().unwrap();
    assert_eq!(
        first.id,
        local_id(&format!("mem_{suffix}"), first.created_at)
    );
    assert_eq!(suffix.len(), 32);
    let second = store
        .supersede("p", &first.id, 1, input(), "New decision", Actor::Human)
        .await
        .unwrap();
    let suffix = second.id.rsplit('_').next().unwrap();
    assert_eq!(
        second.id,
        local_id(&format!("mem_{suffix}"), second.created_at)
    );
    assert_ne!(first.id, second.id);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Verify the complete transaction and its retained references together.
async fn migration_preserves_history_references_rowids_and_cached_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let pool = legacy(&path).await;
    let row: i64 = sqlx::query_scalar("SELECT rowid FROM memories WHERE id=?")
        .bind(NEXT)
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    let store = MemoryStore::open(&path).await.unwrap();
    let old = local_id(OLD, CREATED);
    let next = local_id(NEXT, CREATED);
    let memory = store.show("p", &next, None).await.unwrap();
    assert_eq!(memory.created_at, CREATED);
    assert_eq!(memory.updated_at, CREATED + 1);
    assert_eq!(memory.content, input());
    assert_eq!(memory.supersedes.as_deref(), Some(old.as_str()));
    assert_eq!(
        store
            .show("p", &old, Some(1))
            .await
            .unwrap()
            .superseded_by
            .as_deref(),
        Some(next.as_str())
    );
    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    let actual: i64 = sqlx::query_scalar("SELECT rowid FROM memories WHERE id=?")
        .bind(&next)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row, actual);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    for table in ["memory_vectors", "embedding_failures", "context_deliveries"] {
        let id: String = sqlx::query_scalar(&format!("SELECT memory_id FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(id, next, "{table}");
    }
    let cached: String = sqlx::query_scalar("SELECT data FROM context_receipts")
        .fetch_one(&pool)
        .await
        .unwrap();
    let cached: serde_json::Value = serde_json::from_str(&cached).unwrap();
    assert_eq!(cached["items"][0]["id"], next);
    assert!(cached["text"].as_str().unwrap().contains(&next));
    assert!(cached["text"].as_str().unwrap().contains(OLD));
    let suppressed: String = sqlx::query_scalar("SELECT memory_id FROM suppressions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(suppressed, old);
    let work: (String, i64, Option<String>, String, String) =
        sqlx::query_as("SELECT state,generation,owner,payload,result FROM work_items")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((work.0.as_str(), work.1, work.2), ("pending", 4, None));
    let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM work_items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        attempts, 0,
        "interrupted consolidation keeps its last retry available"
    );
    let payload: serde_json::Value = serde_json::from_str(&work.3).unwrap();
    assert_eq!(payload["review"]["id"], next);
    assert_eq!(
        payload["review"]["content"]["conclusion"],
        input().conclusion
    );
    let decisions: serde_json::Value = serde_json::from_str(&work.4).unwrap();
    assert_eq!(decisions[0]["target"], next);
    assert_eq!(decisions[0]["reason"], OLD);
    let audit: String = sqlx::query_scalar("SELECT data FROM work_audits")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&audit).unwrap()["transcript"],
        OLD
    );
    let source: String = sqlx::query_scalar("SELECT data FROM sources")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&source).unwrap()["messages"][0]["text"],
        OLD
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_fts f JOIN memories m ON m.rowid=f.rowid WHERE m.id=?",
    )
    .bind(&next)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    pool.close().await;
    drop(store);
    let reopened = MemoryStore::open(&path).await.unwrap();
    assert_eq!(reopened.show("p", &next, None).await.unwrap(), memory);
}

#[tokio::test]
async fn projection_migrates_legacy_filename_with_recovery_and_retries_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let path = dir.path().join("memory.db");
    legacy(&path).await.close().await;
    let notes = dir.path().join("Memory");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(
        notes.join(format!("{NEXT}.md")),
        "Legacy note with local edits",
    )
    .unwrap();
    let next = local_id(NEXT, CREATED);
    let target = notes.join(format!("{next}.md"));
    std::fs::write(&target, "Destination belongs to another note").unwrap();
    let store = MemoryStore::open(&path).await.unwrap();
    let projection = MemoryProjection::new(store.clone(), dir.path(), Path::new("Memory")).unwrap();
    let page = projection.sync("p", "", 20).await.unwrap();
    assert!(!page.conflicts.is_empty());
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "Destination belongs to another note"
    );
    assert!(notes.join(format!("{NEXT}.md")).exists());
    std::fs::remove_file(&target).unwrap();
    let page = projection.sync_pending("p", "", 20).await.unwrap();
    assert!(page.conflicts.is_empty(), "{:?}", page.conflicts);
    assert!(!notes.join(format!("{NEXT}.md")).exists());
    assert!(
        std::fs::read_to_string(&target)
            .unwrap()
            .contains(&format!("id: {next}"))
    );
    let recovered = std::fs::read_dir(notes.join("Recovery"))
        .unwrap()
        .map(|f| std::fs::read_to_string(f.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert!(
        recovered
            .iter()
            .any(|s| s == "Legacy note with local edits")
    );
    assert!(
        projection
            .sync_pending("p", "", 20)
            .await
            .unwrap()
            .conflicts
            .is_empty()
    );
}

#[tokio::test]
async fn migration_collision_rolls_back_ids_references_and_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let pool = legacy(&path).await;
    let next = local_id(NEXT, CREATED);
    sqlx::query("INSERT INTO memories(id,project_id,revision,status,data) SELECT ?,'p',1,'active',json_set(data,'$.id',?) FROM memories WHERE id=?")
        .bind(&next).bind(&next).bind(NEXT).execute(&pool).await.unwrap();
    assert!(MemoryStore::open(&path).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM memories WHERE id IN (?,?)")
        .bind(OLD)
        .bind(NEXT)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 1);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("DELETE FROM memories WHERE id=?")
        .bind(&next)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    assert!(MemoryStore::open(&path).await.is_ok());
}
