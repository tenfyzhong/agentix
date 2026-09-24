use super::incremental::connection;
use super::*;
use std::fs;

#[tokio::test]
async fn session_project_lookup_and_registration_ignore_unrelated_projects() {
    let f = Fixture::new().await;
    let other = tempfile::tempdir().unwrap();
    let project = f
        .service
        .execute(
            json!({"command":"project.register","name":"Unrelated","root":other.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query("UPDATE projects SET data=json_remove(data,'$.created_at') WHERE id=?")
        .bind(project)
        .execute(&mut connection(&f).await)
        .await
        .unwrap();
    let result = f
        .service
        .project_for_session(Some(f.dir.path()), None)
        .await;
    assert!(
        result.is_ok(),
        "lookup must not deserialize unrelated Projects: {result:?}"
    );
    assert_eq!(result.unwrap().unwrap().id, f.project);
    let fresh = tempfile::tempdir().unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"project.register","name":"Fresh","root":fresh.path()}),
            WriteOptions::default(),
        )
        .await;
    assert!(
        result.is_ok(),
        "registration must not deserialize unrelated Projects: {result:?}"
    );
}

#[tokio::test]
async fn project_registration_ignores_unrelated_project_bodies() {
    let f = Fixture::new().await;
    sqlx::query("UPDATE projects SET data=json_remove(data,'$.created_at') WHERE id=?")
        .bind(&f.project)
        .execute(&mut connection(&f).await)
        .await
        .unwrap();
    let fresh = tempfile::tempdir().unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"project.register","name":"Fresh","root":fresh.path()}),
            WriteOptions::default(),
        )
        .await;
    assert!(
        result.is_ok(),
        "registration must only read the matching root: {result:?}"
    );
}

#[tokio::test]
async fn project_lookup_migration_builds_indexes_for_existing_projects() {
    let f = Fixture::new().await;
    let mut conn = connection(&f).await;
    sqlx::query("DROP TABLE IF EXISTS project_lookup")
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version=13")
        .execute(&mut conn)
        .await
        .unwrap();
    let mut legacy =
        serde_json::to_value(f.service.store().project_result(&f.project).await.unwrap()).unwrap();
    legacy["id"] = json!("prj_legacy_alias");
    legacy["key"] = json!("Ärea");
    legacy["name"] = json!("Ärea");
    legacy["root"] = json!(f.dir.path().join("."));
    sqlx::query("INSERT INTO projects(id,data) VALUES (?,?)")
        .bind("prj_legacy_alias")
        .bind(legacy.to_string())
        .execute(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let store = Store::open(&f.service.config().storage.path).await.unwrap();
    let mut conn = connection(&f).await;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(version, 14);
    let root = f.dir.path().canonicalize().unwrap();
    assert_eq!(
        store
            .project_by_root(root.to_str().unwrap())
            .await
            .unwrap()
            .unwrap()
            .id,
        f.project
    );
    let folded: String =
        sqlx::query_scalar("SELECT folded_key FROM project_lookup WHERE project_id=?")
            .bind(&f.project)
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(folded, "demo");
    let folded: String = sqlx::query_scalar(
        "SELECT folded_key FROM project_lookup WHERE project_id='prj_legacy_alias'",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(folded, "ärea");
    assert_eq!(store.projects().await.unwrap().len(), 2);
}

#[tokio::test]
async fn project_registration_unicode_collisions_and_deletion_keep_lookup_consistent() {
    let f = Fixture::new().await;
    let mut projects = Vec::new();
    for (index, name) in ["Ärea", "ärea", "Ärea"].iter().enumerate() {
        let root = f.dir.path().join(format!("root-{index}"));
        fs::create_dir(&root).unwrap();
        projects.push(
            f.service
                .execute(
                    json!({"command":"project.register","name":name,"root":root}),
                    WriteOptions::default(),
                )
                .await
                .unwrap()
                .result,
        );
    }
    assert_eq!(projects[0]["key"], "Ärea");
    assert_eq!(projects[1]["key"], "ärea-2");
    assert_eq!(projects[2]["key"], "Ärea-3");
    f.service
        .execute(
            json!({"command":"project.delete","project":projects[1]["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let root = projects[1]["root"].as_str().unwrap();
    assert!(
        f.service
            .store()
            .project_by_root(root)
            .await
            .unwrap()
            .is_none()
    );
    let replacement = f
        .service
        .execute(
            json!({"command":"project.register","name":"ärea","root":root}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(replacement["key"], "ärea-2");
    assert_ne!(replacement["id"], projects[1]["id"]);
}

#[tokio::test]
async fn project_lookup_queries_use_indexes_and_aliases_do_not_create_duplicates() {
    use sqlx::Row;
    let f = Fixture::new().await;
    let mut conn = connection(&f).await;
    for (query, index) in [
        (
            "EXPLAIN QUERY PLAN SELECT p.id FROM project_lookup l JOIN projects p ON p.id=l.project_id WHERE l.canonical_root=? ORDER BY p.rowid LIMIT 1",
            "project_lookup_by_root",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT EXISTS(SELECT 1 FROM project_lookup WHERE folded_key=?)",
            "project_lookup_by_key",
        ),
    ] {
        let rows = sqlx::query(query)
            .bind("demo")
            .fetch_all(&mut conn)
            .await
            .unwrap();
        let plan = rows
            .iter()
            .map(|r| r.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plan.contains(index), "index must serve the query: {plan}");
        assert!(
            !plan.contains("SCAN p") && !plan.contains("SCAN l"),
            "no project scan: {plan}"
        );
    }
    let alias = f.dir.path().join(".");
    let project = f
        .service
        .execute(
            json!({"command":"project.register","name":"Alias","root":alias}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(project["id"], f.project);
    assert_eq!(f.service.store().projects().await.unwrap().len(), 1);
}

#[tokio::test]
async fn project_lookup_failed_migration_rolls_back_and_can_retry() {
    let f = Fixture::new().await;
    let mut conn = connection(&f).await;
    sqlx::query("DROP TABLE project_lookup")
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version=13")
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query("UPDATE projects SET data=json_remove(data,'$.key')")
        .execute(&mut conn)
        .await
        .unwrap();
    assert!(Store::open(&f.service.config().storage.path).await.is_err());
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(version, 13);
    sqlx::query("UPDATE projects SET data=json_set(data,'$.key','demo')")
        .execute(&mut conn)
        .await
        .unwrap();
    assert!(Store::open(&f.service.config().storage.path).await.is_ok());
}
