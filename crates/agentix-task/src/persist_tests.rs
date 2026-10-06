use super::*;

thread_local! {
    pub(crate) static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

async fn fixture() -> (tempfile::TempDir, Store, Snapshot) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let project = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Test"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let job = store
        .execute(
            json!({"command":"job.create","project":project["id"],"title":"Job"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    store
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Task"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = store.snapshot().await.unwrap();
    (dir, store, state)
}

#[tokio::test]
async fn workspace_paths_abbreviate_home_in_storage_and_expand_at_runtime() {
    let (_dir, store, state) = fixture().await;
    let root = dirs::home_dir()
        .unwrap()
        .join("taskix-isolated-workspace-test")
        .to_string_lossy()
        .into_owned();
    let request = json!({"command":"project.register","root":root,"name":"Home"});
    let options = WriteOptions {
        idempotency_key: Some("relative-workspace".into()),
        ..WriteOptions::default()
    };
    let first = store
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    let replay = store.execute(request, options).await.unwrap();
    assert_eq!(first.result, replay.result);
    assert_eq!(replay.result["root"], root);
    let id = first.result["id"].as_str().unwrap();
    assert_eq!(store.project_result(id).await.unwrap().root, root);
    assert_eq!(store.project_by_root(&root).await.unwrap().unwrap().id, id);
    assert_eq!(
        store
            .project_by_root("~/taskix-isolated-workspace-test")
            .await
            .unwrap()
            .unwrap()
            .id,
        id
    );
    assert_eq!(
        store
            .project_result(&state.projects[0].id)
            .await
            .unwrap()
            .root,
        state.projects[0].root
    );
    for query in [
        "SELECT root FROM projects WHERE json_extract(data,'$.name')='Home'",
        "SELECT canonical_root FROM project_lookup WHERE folded_key='home'",
        "SELECT json_extract(result,'$.result.root') FROM idempotency_keys WHERE key='relative-workspace'",
    ] {
        let paths: Vec<String> = sqlx::query_scalar(query)
            .fetch_all(&store.pool)
            .await
            .unwrap();
        assert!(!paths.is_empty(), "{query}");
        assert!(
            paths.iter().all(|path| path.starts_with("~/")),
            "{query}: {paths:?}"
        );
    }
    let raw: String = sqlx::query_scalar("SELECT root FROM projects WHERE id=?")
        .bind(&state.projects[0].id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(raw, state.projects[0].root);
    assert_eq!(
        store
            .project_summaries()
            .await
            .unwrap()
            .iter()
            .find(|p| p.project.id == id)
            .unwrap()
            .project
            .root,
        root
    );
    assert_eq!(
        store
            .browse_task_page(crate::BrowseScope::Project(id), None, 0, 10, 1)
            .await
            .unwrap()
            .project
            .unwrap()
            .root,
        root
    );
    let same = store.execute(json!({"command":"project.register","root":"~/taskix-isolated-workspace-test","name":"Home"}), WriteOptions::default()).await.unwrap();
    assert_eq!(same.result["id"], id);
    assert_eq!(same.result["root"], root);
}

#[tokio::test]
async fn workspace_home_abbreviation_respects_prefix_boundaries() {
    let (_dir, store, _) = fixture().await;
    let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
    for (name, path, expected) in [
        ("Exact home", home.clone(), "~".to_owned()),
        (
            "Sibling",
            format!("{home}-sibling/workspace"),
            format!("{home}-sibling/workspace"),
        ),
    ] {
        let project = store
            .execute(
                json!({"command":"project.register","root":path,"name":name}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let raw: String = sqlx::query_scalar("SELECT root FROM projects WHERE id=?")
            .bind(project.result["id"].as_str().unwrap())
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(raw, expected);
        assert_eq!(
            store
                .project_result(project.result["id"].as_str().unwrap())
                .await
                .unwrap()
                .root,
            path
        );
    }
}

#[tokio::test]
async fn sync_cursor_keys_abbreviate_home_without_resetting_watermarks() {
    let (_dir, store, _) = fixture().await;
    let key = format!(
        "agentix:cursor:{}",
        dirs::home_dir()
            .unwrap()
            .join(".local/share/agentix/state.sqlite3")
            .display()
    );
    store.set_metadata(&key, &json!(2164)).await.unwrap();
    assert_eq!(store.metadata(&key).await.unwrap(), Some(json!(2164)));
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT key FROM projection_state WHERE key LIKE 'agentix:cursor:%'")
            .fetch_all(&store.pool)
            .await
            .unwrap();
    assert_eq!(
        keys,
        ["agentix:cursor:~/.local/share/agentix/state.sqlite3"]
    );
}

#[tokio::test]
async fn schema_nineteen_migrates_workspace_paths_and_preserves_history_and_cursors() {
    let (dir, store, _) = fixture().await;
    store.set_background_maintenance(false);
    let root = dirs::home_dir()
        .unwrap()
        .join("taskix-isolated-workspace-test")
        .to_string_lossy()
        .into_owned();
    store
        .execute(
            json!({"command":"project.register","root":root,"name":"Home"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = store.snapshot().await.unwrap();
    sqlx::query("PRAGMA user_version=19")
        .execute(&store.pool)
        .await
        .unwrap();
    let project = state.projects.iter().find(|p| p.name == "Home").unwrap();
    sqlx::query("UPDATE projects SET data=json_set(data,'$.root',?) WHERE id=?")
        .bind(&project.root)
        .bind(&project.id)
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE project_lookup SET canonical_root=? WHERE project_id=?")
        .bind(&project.root)
        .bind(&project.id)
        .execute(&store.pool)
        .await
        .unwrap();
    let history = json!({"root":project.root,"conversation":[{"text":project.root}]});
    sqlx::query(
        "INSERT INTO idempotency_keys(key,fingerprint,result) VALUES ('legacy-root','unchanged',?)",
    )
    .bind(json!({"result":history}).to_string())
    .execute(&store.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE task_events SET data=json_set(data,'$.payload.root',?) WHERE sequence=1")
        .bind(&project.root)
        .execute(&store.pool)
        .await
        .unwrap();
    let key = format!(
        "agentix:cursor:{}",
        dirs::home_dir()
            .unwrap()
            .join(".local/share/agentix/state.sqlite3")
            .display()
    );
    for (key, watermark) in [
        (&key, 2164),
        (
            &"agentix:cursor:~/.local/share/agentix/state.sqlite3".to_owned(),
            2100,
        ),
    ] {
        sqlx::query("INSERT INTO projection_state(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(key).bind(watermark.to_string()).execute(&store.pool).await.unwrap();
    }
    let migrated = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    assert_eq!(migrated.snapshot().await.unwrap().projects, state.projects);
    assert_eq!(migrated.metadata(&key).await.unwrap(), Some(json!(2164)));
    let raw: String =
        sqlx::query_scalar("SELECT result FROM idempotency_keys WHERE key='legacy-root'")
            .fetch_one(&migrated.pool)
            .await
            .unwrap();
    let raw: Value = serde_json::from_str(&raw).unwrap();
    assert!(!Path::new(raw["result"]["root"].as_str().unwrap()).is_absolute());
    assert_eq!(raw["result"]["conversation"][0]["text"], project.root);
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&migrated.pool)
        .await
        .unwrap();
    assert_eq!(version, 21);
    assert!(
        migrated
            .events(None, 0, 100)
            .await
            .unwrap()
            .iter()
            .any(|event| event.payload["root"] == project.root)
    );
}

#[tokio::test]
async fn home_path_migration_rolls_back_on_incompatible_cursor_collisions() {
    let (dir, store, _) = fixture().await;
    store.set_background_maintenance(false);
    sqlx::query("PRAGMA user_version=19")
        .execute(&store.pool)
        .await
        .unwrap();
    let root = dirs::home_dir()
        .unwrap()
        .join("migration-workspace")
        .to_string_lossy()
        .into_owned();
    sqlx::query("UPDATE projects SET data=json_set(data,'$.root',?)")
        .bind(&root)
        .execute(&store.pool)
        .await
        .unwrap();
    let key = format!(
        "agentix:cursor:{}/state.sqlite3",
        dirs::home_dir().unwrap().display()
    );
    for (key, value) in [
        (&key, "2164"),
        (
            &"agentix:cursor:~/state.sqlite3".to_owned(),
            "\"incompatible\"",
        ),
    ] {
        sqlx::query("INSERT INTO projection_state(key,value) VALUES (?,?)")
            .bind(key)
            .bind(value)
            .execute(&store.pool)
            .await
            .unwrap();
    }
    let error = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("incompatible legacy sync cursors")
    );
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(version, 19);
    let raw: String = sqlx::query_scalar("SELECT root FROM projects")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(raw, root);
    let value: String = sqlx::query_scalar("SELECT value FROM projection_state WHERE key=?")
        .bind(key)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(value, "2164");
}

#[tokio::test]
async fn relative_document_schema_rejects_absolute_paths_from_existing_writers() {
    let (_dir, store, _) = fixture().await;
    for (table, field) in [
        ("projects", "document_directory"),
        ("jobs", "document_path"),
        ("plans", "path"),
    ] {
        for path in ["/old-vault/document.md", "C:/old-vault/document.md"] {
            let data = json!({field: path}).to_string();
            let error = sqlx::query(&format!(
                "INSERT INTO {table}(id,data) VALUES ('old-writer',?)"
            ))
            .bind(data)
            .execute(&store.pool)
            .await
            .expect_err("an already-open old writer must not persist an absolute path");
            assert!(
                error
                    .to_string()
                    .contains("relative document path required")
            );
        }
    }
    for table in ["projects", "jobs"] {
        let field = if table == "projects" {
            "document_directory"
        } else {
            "document_path"
        };
        assert!(
            sqlx::query(&format!(
                "UPDATE {table} SET data=json_set(data,'$.{field}','/old-vault/document.md')"
            ))
            .execute(&store.pool)
            .await
            .is_err()
        );
    }
    assert!(
        sqlx::query(
            "INSERT INTO document_registry(key,path) VALUES ('old-writer','/old-vault/Board.md')"
        )
        .execute(&store.pool)
        .await
        .is_err()
    );
    sqlx::query("INSERT INTO document_registry(key,path) VALUES ('relative','Archived Projects/Test/Board.md')")
        .execute(&store.pool).await.unwrap();
    assert!(
        sqlx::query("UPDATE document_registry SET path='/old-vault/Board.md' WHERE key='relative'")
            .execute(&store.pool)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn schema_eighteen_migrates_cleanup_and_replay_paths_without_changing_workspaces() {
    let (dir, store, state) = fixture().await;
    store.set_background_maintenance(false);
    sqlx::query("PRAGMA user_version=18")
        .execute(&store.pool)
        .await
        .unwrap();
    let project = &state.projects[0];
    let old = "C:/old-vault/History/Projects/Test";
    sqlx::query("UPDATE projects SET data=json_set(data,'$.archived_at',1,'$.document_directory',?) WHERE id=?")
        .bind(old).bind(&project.id).execute(&store.pool).await.unwrap();
    let path = format!("{old}/Jobs/job.md");
    sqlx::query("UPDATE jobs SET data=json_set(data,'$.document_path',?) WHERE id=?")
        .bind(&path)
        .bind(&state.jobs[0].id)
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO document_registry(key,path) VALUES ('legacy-board',?)")
        .bind(format!("{old}/Board.md"))
        .execute(&store.pool)
        .await
        .unwrap();
    let orphan = "C:/old-vault/History/Projects/Deleted";
    let cleanup = json!({"id":"legacy-cleanup","files":[path],"directories":[orphan],"candidates":{format!("{orphan}/Tasks/task.md"): ["task_deleted"]}});
    sqlx::query("INSERT INTO document_deletions(id,data) VALUES ('legacy-cleanup',?)")
        .bind(cleanup.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
    let result = json!({"result":{"document_directory":old,"root":project.root,"conversation":[{"text":old}]}});
    sqlx::query(
        "INSERT INTO idempotency_keys(key,fingerprint,result) VALUES ('legacy-key','unchanged',?)",
    )
    .bind(result.to_string())
    .execute(&store.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE task_events SET data=json_set(data,'$.payload.document_directory',?) WHERE sequence=1")
        .bind(old).execute(&store.pool).await.unwrap();
    sqlx::query("PRAGMA user_version=18")
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;
    store.maintenance_pool.close().await;
    let migrated = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let after = migrated.snapshot().await.unwrap();
    assert_eq!(after.projects[0].root, project.root);
    assert_eq!(after.projects[0].revision, project.revision);
    assert_eq!(after.jobs[0].revision, state.jobs[0].revision);
    assert_eq!(
        after.projects[0].document_directory(),
        "Archived Projects/Test"
    );
    let cleanup: String = sqlx::query_scalar("SELECT data FROM document_deletions")
        .fetch_one(&migrated.pool)
        .await
        .unwrap();
    let cleanup: Value = serde_json::from_str(&cleanup).unwrap();
    assert_eq!(cleanup["files"][0], "Archived Projects/Test/Jobs/job.md");
    assert_eq!(cleanup["directories"][0], "Archived Projects/Deleted");
    assert!(
        cleanup["candidates"]
            .get("Archived Projects/Deleted/Tasks/task.md")
            .is_some()
    );
    let result: String =
        sqlx::query_scalar("SELECT result FROM idempotency_keys WHERE key='legacy-key'")
            .fetch_one(&migrated.pool)
            .await
            .unwrap();
    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        result["result"]["document_directory"],
        "Archived Projects/Test"
    );
    assert_eq!(result["result"]["root"], project.root);
    assert_eq!(result["result"]["conversation"][0]["text"], old);
    assert_eq!(
        migrated.events(None, 0, 10).await.unwrap()[0].payload["document_directory"],
        "Archived Projects/Test"
    );
}

#[tokio::test]
async fn relative_path_migration_rolls_back_the_schema_and_entities_on_invalid_paths() {
    let (dir, store, state) = fixture().await;
    store.set_background_maintenance(false);
    sqlx::query("PRAGMA user_version=18")
        .execute(&store.pool)
        .await
        .unwrap();
    let old = "/old-vault/Archive/Test";
    sqlx::query("UPDATE projects SET data=json_set(data,'$.archived_at',1,'$.document_directory',?) WHERE id=?")
        .bind(old).bind(&state.projects[0].id).execute(&store.pool).await.unwrap();
    sqlx::query("UPDATE jobs SET data=json_set(data,'$.document_path','/old-vault/Archive/Test/../escape.md')")
        .execute(&store.pool).await.unwrap();
    sqlx::query("PRAGMA user_version=18")
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;
    store.maintenance_pool.close().await;
    assert!(
        Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .is_err()
    );
    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new().filename(dir.path().join("tasks.sqlite3")),
    )
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 18);
    let directory: String =
        sqlx::query_scalar("SELECT json_extract(data,'$.document_directory') FROM projects")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(directory, old);
}

#[tokio::test]
async fn schema_fifteen_event_retention_database_upgrades_with_memory_sources() {
    assert_schema_fifteen_upgrade(false).await;
}

#[tokio::test]
async fn schema_fifteen_memory_database_upgrades_with_event_watermarks() {
    assert_schema_fifteen_upgrade(true).await;
}

#[tokio::test]
async fn schema_sixteen_and_seventeen_upgrade_to_relative_document_paths() {
    for previous in [16, 17] {
        let (dir, store, snapshot) = fixture().await;
        sqlx::query(&format!("PRAGMA user_version={previous}"))
            .execute(&store.pool)
            .await
            .unwrap();
        store.pool.close().await;
        store.maintenance_pool.close().await;
        let reopened = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&reopened.pool)
            .await
            .unwrap();
        assert_eq!(version, 21);
        assert_eq!(
            reopened.snapshot().await.unwrap().projects,
            snapshot.projects
        );
    }
}

async fn assert_schema_fifteen_upgrade(has_memory: bool) {
    let (dir, store, snapshot) = fixture().await;
    store.set_background_maintenance(false);
    let project = &snapshot.projects[0].id;
    store.execute(
        json!({"command":"session.record","project":project,"session":"migration","turn_id":"decision","messages":[{"id":"user","role":"user","text":"Keep the external deployment decision"}]}),
        WriteOptions { session_ref: Some("migration".into()), ..WriteOptions::default() },
    ).await.unwrap();
    let sources = store.memory_sources(0, 20).await.unwrap();
    assert_eq!(sources.len(), 1);
    let instance = store.memory_source_instance().await.unwrap();
    let sequence = store.latest_sequence().await.unwrap();
    let project_sequence: i64 =
        sqlx::query_scalar("SELECT sequence FROM event_watermarks WHERE scope=?")
            .bind(project)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    let drops = if has_memory {
        "DROP TRIGGER event_watermark_insert; DROP TABLE event_watermarks; DROP TABLE event_retention; DROP INDEX events_by_age;"
    } else {
        "DROP TABLE memory_source_outbox; DROP TABLE memory_source_heads; DROP TABLE memory_source_identity;"
    };
    sqlx::raw_sql(&format!("{drops} PRAGMA user_version=15;"))
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;
    store.maintenance_pool.close().await;
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    store.set_background_maintenance(false);
    assert_eq!(store.snapshot().await.unwrap().jobs, snapshot.jobs);
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(version, 21);
    assert!(
        store.event_policy(None, None, None).await.unwrap()["enabled"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(store.latest_sequence().await.unwrap(), sequence);
    let restored_sequence: i64 =
        sqlx::query_scalar("SELECT sequence FROM event_watermarks WHERE scope=?")
            .bind(project)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(restored_sequence, project_sequence);
    let new_instance = store.memory_source_instance().await.unwrap();
    assert!(!new_instance.is_empty());
    if has_memory {
        assert_eq!(new_instance, instance);
        assert_eq!(
            serde_json::to_value(store.memory_sources(0, 20).await.unwrap()).unwrap(),
            serde_json::to_value(sources).unwrap()
        );
    } else {
        assert!(store.memory_sources(0, 20).await.unwrap().is_empty());
    }
    let read_only = Store::open_read_only(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    assert_eq!(
        read_only.project_result(project).await.unwrap().id,
        *project
    );
}

#[tokio::test]
async fn wide_unchanged_state_does_not_rescan_entities_for_each_row() {
    let (_dir, store, mut state) = fixture().await;
    let job = state.jobs[0].clone();
    let task = state.tasks[0].clone();
    for index in 0..1000 {
        let mut job = job.clone();
        job.id = format!("job_wide_{index}");
        let mut task = task.clone();
        task.id = format!("task_wide_{index}");
        task.job_id.clone_from(&job.id);
        state.jobs.push(job);
        state.tasks.push(task);
    }
    let sequence = store.latest_sequence().await.unwrap();
    let mut tx = store.pool.begin().await.unwrap();
    VISITS.set(0);
    persist(
        &mut tx,
        &state,
        &state,
        "project.archive",
        &WriteOptions::default(),
        store.now(),
    )
    .await
    .unwrap();
    let visits = VISITS.get();
    eprintln!("Wide unchanged-state entity visits: {visits}");
    assert!(visits > 0, "work counter must include index construction");
    tx.commit().await.unwrap();
    assert_eq!(store.latest_sequence().await.unwrap(), sequence);
    assert!(
        visits < state.tasks.len() * 16,
        "unchanged state comparison visited {visits} identifiers for {} tasks",
        state.tasks.len()
    );
}

#[tokio::test]
async fn indexed_changes_keep_event_order_and_latest_session_ties() {
    use crate::{JobStatus, TaskStatus};
    for explicit in [None, Some("explicit")] {
        let (_dir, store, mut before) = fixture().await;
        for (index, session, updated) in
            [(1, Some("first"), 10), (2, Some("last"), 10), (3, None, 20)]
        {
            let mut task = before.tasks[0].clone();
            task.id = format!("task_event_{index}");
            task.last_session = session.map(str::to_owned);
            task.updated_at = updated;
            before.tasks.push(task);
        }
        before.jobs[0].status = JobStatus::PendingReview;
        let mut after = before.clone();
        after.jobs[0].status = JobStatus::Active;
        after.jobs[0].revision += 1;
        after.tasks[0].status = TaskStatus::Blocked;
        after.tasks[0].revision += 1;
        let sequence = store.latest_sequence().await.unwrap();
        let options = WriteOptions {
            session_ref: explicit.map(str::to_owned),
            ..WriteOptions::default()
        };
        let mut tx = store.pool.begin().await.unwrap();
        persist(
            &mut tx,
            &before,
            &after,
            "inbox.set-status",
            &options,
            store.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let events = store.events(None, sequence, 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "job.rejected");
        assert_eq!(
            events[0].session_ref.as_deref(),
            Some(explicit.unwrap_or("last"))
        );
        assert_eq!(events[1].event_type, "task.blocked");
        assert_eq!(
            events[1].task_id.as_deref(),
            Some(after.tasks[0].id.as_str())
        );
        assert_eq!(events[1].session_ref.as_deref(), explicit);
    }
}

#[tokio::test]
async fn events_do_not_duplicate_job_bodies_and_keep_notification_fields() {
    let (_dir, store, state) = fixture().await;
    let job = &state.jobs[0];
    store.execute(json!({"command":"job.update","job":job.id,"prompt":"large prompt".repeat(1000),"title":"Notice title"}), WriteOptions::default()).await.unwrap();
    let events = store.events(Some(&job.id), 0, 100).await.unwrap();
    let event = events.last().unwrap();
    assert_eq!(event.payload["title"], "Notice title");
    assert!(
        event.payload.get("prompt").is_none(),
        "events must not duplicate authored bodies"
    );
    assert!(event.payload.get("conversation").is_none());
    assert!(serde_json::to_vec(event).unwrap().len() < 2048);
    assert_eq!(store.snapshot().await.unwrap().jobs[0].prompt.len(), 12000);
}

#[tokio::test]
async fn deleting_retained_events_does_not_regress_global_or_project_receipts() {
    let (_dir, store, state) = fixture().await;
    let latest = store.latest_sequence().await.unwrap();
    let receipt = store.project_receipt(&state.projects[0]).await.unwrap();
    sqlx::query("DELETE FROM task_events")
        .execute(&store.pool)
        .await
        .unwrap();
    assert_eq!(store.latest_sequence().await.unwrap(), latest);
    assert_eq!(
        store.project_receipt(&state.projects[0]).await.unwrap(),
        receipt
    );
    let outcome = store
        .execute(
            json!({"command":"job.update","job":state.jobs[0].id,"title":"Later"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(outcome.sequence > latest);
}
