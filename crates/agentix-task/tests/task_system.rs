use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use agentix_task::{Config, Service, Store, WriteOptions};
use serde_json::{Value, json};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    service: Service,
    clock: Arc<AtomicI64>,
    project: String,
    job: String,
}

impl Fixture {
    async fn new(format: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("documents");
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        let config: Config = toml::from_str(&format!(
            "schema_version = 1\n[storage]\npath = {:?}\n[documents]\nformat = {format:?}\nroot = {:?}\ndirectory = 'Tasks 中文'\n",
            dir.path().join("tasks.sqlite3").to_str().unwrap(), root.to_str().unwrap()
        )).unwrap();
        let clock = Arc::new(AtomicI64::new(1_788_566_400));
        let now = clock.clone();
        let store = Store::open_with_clock(
            &config.storage.path,
            Arc::new(move || now.load(Ordering::SeqCst)),
        )
        .await
        .unwrap();
        let service = Service::new(config, store).unwrap();
        let project = service
            .execute(
                json!({"command":"project.register","name":"demo", "root":dir.path()}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let job = service.execute(json!({"command":"job.create","project":project,"title":"Feature","goal":"Ship it"}), WriteOptions::default()).await.unwrap().result["id"].as_str().unwrap().to_owned();
        Self {
            dir,
            service,
            clock,
            project,
            job,
        }
    }

    async fn task(&self, title: &str) -> String {
        self.service
            .execute(
                json!({"command":"task.add","job":self.job,"title":title}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn plan(&self, task: &str) -> Value {
        self.service.execute(json!({"command":"plan.create","task":task,"body":"# Plan\n\nImplement and verify.\n"}), WriteOptions::default()).await.unwrap().result
    }

    async fn claim(&self, task: &str, session: &str) -> Value {
        self.service.execute(json!({"command":"task.claim","task":task,"executor":format!("agent:{session}"),"session":session,"delegated_by":"team:example"}), WriteOptions::default()).await.unwrap().result
    }
}

fn owner(claim: &Value) -> WriteOptions {
    WriteOptions {
        actor_ref: claim["lease"]["executor_ref"].as_str().unwrap().into(),
        session_ref: claim["lease"]["session_ref"].as_str().map(str::to_owned),
        lease_token: claim["lease"]["token"].as_str().map(str::to_owned),
        ..WriteOptions::default()
    }
}

#[tokio::test]
async fn claims_require_plans_dependencies_and_exclusive_session_ownership() {
    let f = Fixture::new("markdown").await;
    let a = f.task("database").await;
    let b = f.task("client").await;
    let cmd = json!({"command":"task.claim","task":a,"executor":"agent:a","session":"a"});
    assert!(
        f.service
            .execute(cmd.clone(), WriteOptions::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("Plan")
    );
    f.plan(&a).await;
    f.plan(&b).await;
    f.service
        .execute(
            json!({"command":"task.depend","task":b,"dependency":a}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"task.depend","task":a,"dependency":b}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    assert!(
        f.service
            .execute(
                json!({"command":"task.claim","task":b,"executor":"agent:b","session":"b"}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    let claim = f
        .service
        .execute(cmd, WriteOptions::default())
        .await
        .unwrap()
        .result;
    assert!(
        f.service
            .execute(
                json!({"command":"task.claim","task":a,"executor":"agent:b","session":"b"}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    assert!(
        f.service
            .execute(
                json!({"command":"task.done","task":a}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    f.service
        .execute(json!({"command":"task.done","task":a}), owner(&claim))
        .await
        .unwrap();
    f.claim(&b, "b").await;
    assert!(
        f.service
            .execute(
                json!({"command":"task.reopen","task":a}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn independent_connections_racing_to_claim_have_one_winner() {
    let f = Fixture::new("markdown").await;
    let id = f.task("exclusive").await;
    f.plan(&id).await;
    let now = f.clock.clone();
    let other = Store::open_with_clock(
        &f.service.config().storage.path,
        Arc::new(move || now.load(Ordering::SeqCst)),
    )
    .await
    .unwrap();
    let first = f.service.store().execute(
        json!({"command":"task.claim","task":id,"executor":"agent:a","session":"a"}),
        WriteOptions::default(),
    );
    let second = other.execute(
        json!({"command":"task.claim","task":id,"executor":"agent:b","session":"b"}),
        WriteOptions::default(),
    );
    let (a, b) = tokio::join!(first, second);
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(f.service.store().snapshot().await.unwrap().leases.len(), 1);
}

#[tokio::test]
async fn stale_lease_cannot_heartbeat_or_complete_after_reclaim() {
    let f = Fixture::new("markdown").await;
    let task = f.task("leased").await;
    f.plan(&task).await;
    let old = f.claim(&task, "old").await;
    f.clock.fetch_add(3600, Ordering::SeqCst);
    f.service.store().reap_expired().await.unwrap();
    assert_eq!(
        f.service.store().snapshot().await.unwrap().tasks[0]
            .status
            .to_string(),
        "BLOCKED"
    );
    let new = f.claim(&task, "new").await;
    for command in ["task.heartbeat", "task.done"] {
        assert!(
            f.service
                .execute(json!({"command":command,"task":task}), owner(&old))
                .await
                .is_err()
        );
    }
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&new))
        .await
        .unwrap();
    assert_eq!(
        f.service.store().snapshot().await.unwrap().jobs[0]
            .status
            .to_string(),
        "COMPLETED"
    );
}

#[tokio::test]
async fn idempotency_and_revisions_prevent_replay_and_lost_updates() {
    let f = Fixture::new("markdown").await;
    let cmd = json!({"command":"task.add","job":f.job,"title":"once"});
    let options = WriteOptions {
        idempotency_key: Some("create-once".into()),
        ..WriteOptions::default()
    };
    let first = f
        .service
        .execute(cmd.clone(), options.clone())
        .await
        .unwrap();
    let again = f.service.execute(cmd, options.clone()).await.unwrap();
    assert_eq!(first.result, again.result);
    assert!(
        f.service
            .execute(
                json!({"command":"task.add","job":f.job,"title":"different"}),
                options
            )
            .await
            .is_err()
    );
    let task = first.result["id"].as_str().unwrap();
    let opts = WriteOptions {
        expected_revision: Some(0),
        ..WriteOptions::default()
    };
    assert!(
        f.service
            .execute(
                json!({"command":"task.update","task":task,"title":"stale"}),
                opts
            )
            .await
            .is_err()
    );
    assert_eq!(f.service.store().snapshot().await.unwrap().tasks.len(), 1);
    let events = f
        .service
        .store()
        .events(Some(&f.job), 0, 100)
        .await
        .unwrap();
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    let cursor = events.last().unwrap().sequence;
    assert!(
        f.service
            .store()
            .events(Some(&f.job), cursor, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn session_resume_only_recovers_system_blocks_and_preserves_team_origin() {
    let f = Fixture::new("markdown").await;
    let id = f.task("resume").await;
    f.plan(&id).await;
    f.claim(&id, "codex:one").await;
    f.service
        .execute(
            json!({"command":"session.end","session":"codex:one"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"session.start","session":"codex:one"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.tasks[0].status.to_string(), "IN_PROGRESS");
    assert_eq!(
        state.leases[0].delegated_by.as_deref(),
        Some("team:example")
    );
    let token = state.leases[0].token.clone();
    let opts = WriteOptions {
        actor_ref: "agent:codex:one".into(),
        session_ref: Some("codex:one".into()),
        lease_token: Some(token),
        ..WriteOptions::default()
    };
    f.service
        .execute(
            json!({"command":"task.block","task":id,"reason":"Need input"}),
            opts,
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"session.start","session":"codex:one"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.service.store().snapshot().await.unwrap().tasks[0]
            .status
            .to_string(),
        "BLOCKED"
    );
}

#[tokio::test]
async fn cancelled_only_jobs_are_not_completed_and_finished_jobs_reject_new_tasks() {
    let f = Fixture::new("markdown").await;
    let id = f.task("cancelled").await;
    f.service
        .execute(
            json!({"command":"task.cancel","task":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.service.store().snapshot().await.unwrap().jobs[0]
            .status
            .to_string(),
        "ACTIVE"
    );
    f.service
        .execute(
            json!({"command":"task.reopen","task":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.plan(&id).await;
    let claim = f.claim(&id, "s").await;
    f.service
        .execute(json!({"command":"task.done","task":id}), owner(&claim))
        .await
        .unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"task.add","job":f.job,"title":"new scope"}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn projections_are_read_only_preserve_notes_and_archive_links() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        let id = f.task("Task | 中文 [x]").await;
        let plan = f.plan(&id).await;
        let state = f.service.store().snapshot().await.unwrap();
        let project = &state.projects[0];
        let output = f.service.config().output_dir();
        let job_path = output.join(format!("Projects/{}/Jobs/Active/{}.md", project.key, f.job));
        let job = std::fs::read_to_string(&job_path).unwrap();
        assert!(job.contains("GENERATED"));
        let changed = job.replace(
            "<!-- taskcli:notes:start -->",
            "<!-- taskcli:notes:start -->\nMy persistent note.",
        );
        std::fs::write(&job_path, changed).unwrap();
        let board_path = output.join(format!("Projects/{}/Board.md", project.key));
        std::fs::write(&board_path, "manual state edit DONE").unwrap();
        f.service.sync().await.unwrap();
        let board = std::fs::read_to_string(&board_path).unwrap();
        assert!(!board.contains("manual state edit"));
        assert!(!board.contains("- [ ]"));
        assert!(!board.contains("kanban-plugin"));
        assert_eq!(board.contains("[["), format == "obsidian");
        if format == "markdown" {
            assert!(board.contains("](Plans/"));
        }
        assert!(
            std::fs::read_to_string(&job_path)
                .unwrap()
                .contains("My persistent note.")
        );
        let plan_path = std::path::PathBuf::from(plan["absolute_path"].as_str().unwrap());
        std::fs::write(&plan_path, "# My revised plan\n").unwrap();
        f.service.sync().await.unwrap();
        let before = f.service.store().snapshot().await.unwrap().plans[0]
            .hash
            .clone();
        let revised = f
            .service
            .execute(
                json!({"command":"plan.revise","task":id,"body":"# Second version\n"}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(revised.result["version"], 2);
        assert_eq!(
            std::fs::read_to_string(&plan_path).unwrap(),
            "# My revised plan\n"
        );
        assert!(!before.is_empty());
        let claim = f.claim(&id, "archive").await;
        f.service
            .execute(json!({"command":"task.done","task":id}), owner(&claim))
            .await
            .unwrap();
        f.service
            .execute(
                json!({"command":"job.archive","job":f.job}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let archived = f.service.store().snapshot().await.unwrap().jobs[0]
            .document_path
            .clone();
        assert!(archived.contains("/Archive/2026/09/"));
        assert!(!job_path.exists());
        assert!(
            std::fs::read_to_string(output.join(archived))
                .unwrap()
                .contains("My persistent note.")
        );
        f.service
            .execute(
                json!({"command":"job.unarchive","job":f.job}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        assert!(job_path.exists());
    }
}

#[tokio::test]
async fn concurrent_jobs_are_independent_and_cross_project_dependencies_are_rejected() {
    let f = Fixture::new("markdown").await;
    let a = f.task("a").await;
    let j = f.service.execute(json!({"command":"job.create","project":f.project,"title":"Next requirement","goal":"Independent"}), WriteOptions::default()).await.unwrap().result["id"].as_str().unwrap().to_owned();
    let b = f
        .service
        .execute(
            json!({"command":"task.add","job":j,"title":"b"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.plan(&a).await;
    f.plan(&b).await;
    f.claim(&a, "a").await;
    f.claim(&b, "b").await;
    assert_eq!(f.service.store().snapshot().await.unwrap().leases.len(), 2);
    let other = f
        .service
        .execute(
            json!({"command":"project.register","name":"other","root":f.dir.path().join("other")}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let job = f
        .service
        .execute(
            json!({"command":"job.create","project":other["id"],"title":"Other","goal":"Other"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let task = f
        .service
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"other"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert!(
        f.service
            .execute(
                json!({"command":"task.depend","task":task["id"],"dependency":a}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn obsidian_alias_separator_is_escaped_only_inside_tables() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Linked task").await;
    f.plan(&task).await;
    let state = f.service.store().snapshot().await.unwrap();
    let output = f.service.config().output_dir();
    let dashboard = std::fs::read_to_string(output.join("Dashboard.md")).unwrap();
    assert!(dashboard.contains("|Feature]]"));
    assert!(!dashboard.contains("\\|Feature]]"));
    let board = std::fs::read_to_string(
        output.join(format!("Projects/{}/Board.md", state.projects[0].key)),
    )
    .unwrap();
    assert!(board.contains("\\|Linked task]]"));
}

#[tokio::test]
async fn plan_idempotent_replay_returns_the_same_result_and_path() {
    let f = Fixture::new("markdown").await;
    let task = f.task("idempotent plan").await;
    let request = json!({"command":"plan.create","task":task,"body":"# Plan"});
    let options = WriteOptions {
        idempotency_key: Some("plan-once".into()),
        ..WriteOptions::default()
    };
    let first = f
        .service
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    let second = f.service.execute(request, options).await.unwrap();
    assert_eq!(first.result, second.result);
    assert_eq!(f.service.store().snapshot().await.unwrap().plans.len(), 1);
}

#[tokio::test]
async fn direct_store_calls_also_reject_expired_lease_tokens() {
    let f = Fixture::new("markdown").await;
    let task = f.task("expired").await;
    f.plan(&task).await;
    let claim = f.claim(&task, "expired").await;
    f.clock.fetch_add(3600, Ordering::SeqCst);
    assert!(
        f.service
            .store()
            .execute(json!({"command":"task.done","task":task}), owner(&claim))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn missing_plan_prevents_automatic_session_resume() {
    let f = Fixture::new("markdown").await;
    let task = f.task("missing plan").await;
    let plan = f.plan(&task).await;
    f.claim(&task, "missing").await;
    f.service
        .execute(
            json!({"command":"session.end","session":"missing"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    std::fs::rename(
        plan["absolute_path"].as_str().unwrap(),
        f.dir.path().join("preserved-plan.md"),
    )
    .unwrap();
    let _ = f
        .service
        .execute(
            json!({"command":"session.start","session":"missing"}),
            WriteOptions::default(),
        )
        .await;
    assert_eq!(
        f.service.store().snapshot().await.unwrap().tasks[0].status,
        agentix_task::TaskStatus::Blocked
    );
}

#[tokio::test]
async fn projection_failure_does_not_lose_committed_task_and_sync_repairs_it() {
    let f = Fixture::new("markdown").await;
    let output = f.service.config().output_dir();
    std::fs::rename(
        output.join("Dashboard.md"),
        f.dir.path().join("saved-dashboard.md"),
    )
    .unwrap();
    std::fs::create_dir(output.join("Dashboard.md")).unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"task.add","job":f.job,"title":"Committed"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.projection_pending.is_some());
    assert_eq!(f.service.store().snapshot().await.unwrap().tasks.len(), 1);
    std::fs::remove_dir(output.join("Dashboard.md")).unwrap();
    f.service.sync().await.unwrap();
    assert!(output.join("Dashboard.md").is_file());
}

#[tokio::test]
async fn concurrent_projections_keep_all_tasks_and_editable_notes() {
    let f = Fixture::new("markdown").await;
    let request_a = json!({"command":"task.add","job":f.job,"title":"Parallel A"});
    let request_b = json!({"command":"task.add","job":f.job,"title":"Parallel B"});
    let other = Service::open(f.service.config().clone()).await.unwrap();
    let (a, b) = tokio::join!(
        f.service.execute(request_a, WriteOptions::default()),
        other.execute(request_b, WriteOptions::default())
    );
    assert!(a.unwrap().projection_pending.is_none());
    assert!(b.unwrap().projection_pending.is_none());
    let state = f.service.store().snapshot().await.unwrap();
    let path = f
        .service
        .config()
        .output_dir()
        .join(&state.jobs[0].document_path);
    let body = std::fs::read_to_string(path).unwrap();
    assert!(body.contains("Parallel A") && body.contains("Parallel B"));
}

#[tokio::test]
async fn state_machine_rejects_skipped_work_and_requires_explicit_retry_and_reopen() {
    let f = Fixture::new("markdown").await;
    let task = f.task("workflow").await;
    assert!(
        f.service
            .execute(
                json!({"command":"task.done","task":task}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    f.plan(&task).await;
    let claim = f.claim(&task, "workflow").await;
    f.service
        .execute(
            json!({"command":"task.fail","task":task,"reason":"test failed"}),
            owner(&claim),
        )
        .await
        .unwrap();
    assert!(f.service.execute(json!({"command":"task.claim","task":task,"session":"workflow","executor":"agent:w"}),WriteOptions::default()).await.is_err());
    f.service
        .execute(
            json!({"command":"task.retry","task":task}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let claim = f.claim(&task, "workflow").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"task.retry","task":task}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    f.service
        .execute(
            json!({"command":"task.reopen","task":task}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.service.store().snapshot().await.unwrap().jobs[0].status,
        agentix_task::JobStatus::Active
    );
}

#[tokio::test]
async fn archive_repair_preserves_notes_after_old_file_was_removed() {
    let f = Fixture::new("markdown").await;
    let task = f.task("archive recovery").await;
    f.plan(&task).await;
    let claim = f.claim(&task, "archive-recovery").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let output = f.service.config().output_dir();
    let job = f.service.store().snapshot().await.unwrap().jobs[0].clone();
    let path = output.join(&job.document_path);
    let body = std::fs::read_to_string(&path).unwrap().replace(
        "<!-- taskcli:notes:start -->",
        "<!-- taskcli:notes:start -->\nKeep these acceptance notes.",
    );
    std::fs::write(&path, body).unwrap();
    let old_paths = f
        .service
        .store()
        .metadata("documents")
        .await
        .unwrap()
        .unwrap();
    f.service
        .execute(
            json!({"command":"job.archive","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    // Simulate a crash after moving documents, before acknowledging the new paths.
    f.service
        .store()
        .set_metadata("documents", &old_paths)
        .await
        .unwrap();
    assert!(!path.exists());
    f.service.sync().await.unwrap();
    let job = f.service.store().snapshot().await.unwrap().jobs[0].clone();
    assert!(
        std::fs::read_to_string(output.join(job.document_path))
            .unwrap()
            .contains("Keep these acceptance notes.")
    );
}

#[tokio::test]
async fn dependency_projection_has_no_trailing_whitespace() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        let first = f.task("prerequisite").await;
        let second = f.task("dependent").await;
        f.service
            .execute(
                json!({"command":"task.depend","task":second,"dependency":first}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let job = f.service.store().snapshot().await.unwrap().jobs[0].clone();
        let body = std::fs::read_to_string(f.service.config().output_dir().join(job.document_path))
            .unwrap();
        let dependencies = body
            .lines()
            .find(|line| line.starts_with("Dependencies:"))
            .unwrap();
        assert_eq!(dependencies, dependencies.trim_end());
    }
}

#[tokio::test]
async fn task_database_rejects_an_unrelated_sqlite_database() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("runtime.sqlite3");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::query("CREATE TABLE bindings (session_id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    assert!(Store::open(&path).await.is_err());
}

#[tokio::test]
async fn invalid_document_configuration_is_rejected_before_writing() {
    let f = Fixture::new("markdown").await;
    let mut config = f.service.config().clone();
    config.documents.directory = "../outside".into();
    assert!(config.validate().is_err());
    let mut config = f.service.config().clone();
    config.storage.path = config.output_dir().join("state.sqlite3");
    assert!(config.validate().is_err());
    let mut config = f.service.config().clone();
    config.schema_version = 2;
    assert!(config.validate().is_err());
}
