use super::*;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

#[tokio::test]
async fn project_and_job_reads_ignore_unrelated_entities() {
    let cli = Cli::new("markdown");
    let job = cli.job("Selected Job");
    let task = cli.task(&job, "Selected Task");
    let project = cli.ok(&["job", "show", &job])["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let commands = [
        vec!["project", "show", &project],
        vec!["project", "list"],
        vec!["job", "show", &job],
        vec!["job", "list", "--project", &project],
    ];
    let expected: Vec<_> = commands.iter().map(|args| cli.ok(args)).collect();
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(cli.dir.path().join("state.sqlite3")),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(&task)
        .execute(&mut db)
        .await
        .unwrap();
    for (args, expected) in commands.iter().zip(expected) {
        assert_eq!(cli.ok(args), expected, "{args:?}");
    }
}

#[tokio::test]
async fn filtered_task_list_ignores_unrelated_bodies_and_checks_external_dependencies() {
    let cli = Cli::new("markdown");
    let job = cli.job("Selected Job");
    let task = cli.task(&job, "Selected Task");
    let other_job = cli.job("Other Job");
    let dependency = cli.task(&other_job, "External dependency");
    cli.ok(&["task", "depend", &task, &dependency]);
    let all = cli.ok(&["task", "list", "--job", &job]);
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(cli.dir.path().join("state.sqlite3")),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(&dependency)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(&other_job)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(cli.ok(&["task", "list", "--job", &job]), all);
    assert_eq!(
        cli.ok(&["task", "list", "--job", &job, "--ready"]),
        json!([])
    );
    sqlx::query("UPDATE tasks SET data=json_set(data,'$.status','DONE') WHERE id=?")
        .bind(&dependency)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(cli.ok(&["task", "list", "--job", &job, "--ready"]), all);
}

#[tokio::test]
async fn context_ignores_unrelated_history_and_plan_bodies() {
    let cli = Cli::new("markdown");
    let old_job = cli.job("Old Job");
    let old_task = cli.task(&old_job, "Old Task");
    let old_claim = cli.claim(&old_task, "lookup");
    cli.owned(
        &["plan", "create", &old_task, "--body", "Old plan"],
        &old_claim,
    );
    cli.owned(&["task", "start", &old_task], &old_claim);
    cli.owned(&["task", "done", &old_task], &old_claim);
    let job = cli.job("Current Job");
    let task = cli.task(&job, "Current Task");
    let claim = cli.claim(&task, "lookup");
    cli.owned(&["plan", "create", &task, "--body", "Current plan"], &claim);
    let active = cli.ok(&["context", "--session", "lookup"]);
    let explicit = cli.ok(&["context", "--task", &task]);
    cli.owned(&["task", "start", &task], &claim);
    cli.owned(&["task", "done", &task], &claim);
    let idle = cli.ok(&["context", "--session", "lookup"]);
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(cli.dir.path().join("state.sqlite3")),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(&old_job)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(&old_task)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE plans SET data=json_remove(data,'$.hash') WHERE task_id=?")
        .bind(&old_task)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(cli.ok(&["context", "--session", "lookup"]), idle);
    // Restore the current lease and task to compare active and explicit contexts.
    sqlx::query("UPDATE tasks SET data=? WHERE id=?")
        .bind(active["task"].to_string())
        .bind(&task)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO task_leases(id,data) VALUES (?,?)")
        .bind(&task)
        .bind(active["lease"].to_string())
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(cli.ok(&["context", "--session", "lookup"]), active);
    assert_eq!(cli.ok(&["context", "--task", &task]), explicit);
}
