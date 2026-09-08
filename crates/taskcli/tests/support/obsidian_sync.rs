use super::*;

#[tokio::test]
async fn cli_writes_resolve_projects_without_loading_other_jobs() {
    use sqlx::Connection;
    let cli = Cli::new();
    let job = cli.job("Target");
    let task = cli.task(&job, "Task");
    let project = cli.ok(&["job", "show", &job])["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = cli.job("Unrelated");
    let mut db = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(cli.dir.path().join("state.sqlite3")),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(other)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        cli.ok(&["task", "block", &task, "--reason", "Blocked"])["status"],
        "BLOCKED"
    );
    assert_eq!(
        cli.ok(&["job", "create", "--project", &project, "--title", "New job"])["title"],
        "New job"
    );
}

#[tokio::test]
async fn doctor_reports_pending_documents_even_without_new_events() {
    use sqlx::Connection;
    let cli = Cli::new();
    let job = cli.job("Target");
    let mut db = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(cli.dir.path().join("state.sqlite3")),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO pending_documents(key,generation) VALUES (?,'repair')")
        .bind(format!("job:{job}"))
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(cli.ok(&["doctor"])["healthy"], false);
    cli.ok(&["sync", "--pending"]);
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
}

#[tokio::test]
async fn id_queries_ignore_unrelated_records_and_preserve_task_show_contract() {
    use sqlx::Connection;
    let cli = Cli::new();
    let job = cli.job("Point lookup");
    let task = cli.task(&job, "Point task");
    let project = cli.ok(&["job", "show", &job])["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let entry = cli.ok(&[
        "inbox",
        "add",
        "--project",
        &project,
        "--content",
        "Point Inbox",
    ]);
    let claim = cli.claim(&task, "lookup");
    let expected = cli.ok(&["task", "show", &task]);
    let snapshot = cli.ok(&["obsidian", "snapshot"]);
    let mut db = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(cli.dir.path().join("state.sqlite3")),
    )
    .await
    .unwrap();
    // Valid database rows that cannot deserialize as entities prove these reads
    // never load unrelated tables or the entire tasks table.
    for table in ["tasks", "jobs", "projects", "inbox_entries", "plans"] {
        sqlx::query(&format!("INSERT INTO {table}(id,data) VALUES (?, '{{}}')"))
            .bind(format!(
                "{}_ffffffffffffffffffffffffffffffff",
                table.trim_end_matches('s')
            ))
            .execute(&mut db)
            .await
            .unwrap();
    }
    assert_eq!(cli.ok(&["task", "show", &task]), expected);
    assert_eq!(cli.ok(&["task", "show", &task[..task.len() - 1]]), expected);
    assert_eq!(expected["lease"]["token"], claim["lease"]["token"]);
    let ambiguous = cli.run(&["task", "show", "task_"]);
    assert_eq!(ambiguous.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&ambiguous.stdout).contains("ambiguous"));
    for id in [task.as_str(), job.as_str(), entry["id"].as_str().unwrap()] {
        let note = snapshot["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .unwrap();
        assert_eq!(cli.ok(&["obsidian", "show", id]), *note);
    }
    assert!(cli.ok(&["obsidian", "show", "task_missing"]).is_null());
    assert_eq!(
        cli.run(&["task", "show", "task_missing"]).status.code(),
        Some(1)
    );
    // Single-record reads report stored state without global expiry mutations.
    sqlx::query(
        "UPDATE task_leases SET data = json_set(data, '$.lease_expires_at', 0) WHERE id = ?",
    )
    .bind(&task)
    .execute(&mut db)
    .await
    .unwrap();
    assert_eq!(cli.ok(&["task", "show", &task])["status"], "IN_PROGRESS");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_leases WHERE id = ?")
        .bind(&task)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn obsidian_connection_reads_configuration_without_opening_database() {
    let cli = Cli::new();
    std::fs::write(cli.dir.path().join("state.sqlite3"), b"not a database").unwrap();
    let connection = cli.ok(&["obsidian", "connection"]);
    assert_eq!(connection["protocol_version"], 1);
    assert_eq!(connection["documents"]["directory"], "Tasks \u{2603}");
    assert!(connection.get("notes").is_none());
}

#[test]
fn obsidian_snapshot_identifies_unplanned_renamed_and_archived_notes_without_credentials() {
    let cli = Cli::new();
    let job = cli.job("Status bridge");
    let task = cli.task(&job, "Unplanned");
    let before = cli.ok(&["obsidian", "snapshot"]);
    assert!(before["documents"].get("format").is_none());
    let find = |snapshot: &Value, id: &str| {
        snapshot["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .unwrap()
            .clone()
    };
    let note = find(&before, &task);
    assert_eq!(note["kind"], "task");
    assert_eq!(note["status"], "TODO");
    assert!(
        note["path"]
            .as_str()
            .unwrap()
            .starts_with("Tasks \u{2603}/Projects/Demo/Tasks/")
    );
    assert!(
        cli.dir
            .path()
            .join("vault")
            .join(note["path"].as_str().unwrap())
            .is_file()
    );
    let claim = cli.claim(&task, "bridge");
    let snapshot = cli.ok(&["obsidian", "snapshot"]);
    assert!(
        !snapshot
            .to_string()
            .contains(claim["lease"]["token"].as_str().unwrap())
    );
    cli.owned(&["task", "cancel", &task], &claim);
    cli.ok(&["task", "update", &task, "--name", "Renamed"]);
    cli.ok(&["job", "cancel", &job]);
    cli.ok(&["job", "archive", &job]);
    let after = cli.ok(&["obsidian", "snapshot"]);
    assert!(
        find(&after, &task)["path"]
            .as_str()
            .unwrap()
            .ends_with("-Renamed.md")
    );
    assert!(
        find(&after, &job)["path"]
            .as_str()
            .unwrap()
            .contains("/Jobs/Archived/")
    );
    assert_eq!(find(&after, &task)["properties"]["status"], "CANCELLED");
    assert_eq!(cli.ok(&["obsidian", "show", &task]), find(&after, &task));
    assert_eq!(cli.ok(&["obsidian", "show", &job]), find(&after, &job));
    assert!(find(&after, &task)["revision"].as_i64().unwrap() > note["revision"].as_i64().unwrap());
}
