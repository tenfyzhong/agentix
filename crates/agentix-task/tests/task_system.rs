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
        let claimed = self
            .service
            .store()
            .snapshot()
            .await
            .unwrap()
            .task_result(task)
            .unwrap();
        self.service.execute(json!({"command":"plan.create","task":task,"body":"# Plan\n\nImplement and verify.\n"}), owner(&claimed)).await.unwrap().result
    }

    async fn start(&self, task: &str, session: &str) -> Value {
        let claimed = self.claim(task, session).await;
        if claimed["current_plan"].is_null() {
            self.plan(task).await;
        }
        self.service
            .execute(json!({"command":"task.start","task":task}), owner(&claimed))
            .await
            .unwrap()
            .result
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
async fn start_waits_for_plan_writes_and_rechecks_ownership_before_committing() {
    let f = Fixture::new("markdown").await;
    let task = f.task("serialize start with Plan").await;
    let claim = f.claim(&task, "locking").await;
    f.plan(&task).await;
    let lock = std::fs::File::open(f.service.config().output_dir().join(".taskcli.lock")).unwrap();
    lock.lock().unwrap();
    let service = f.service.clone();
    let task_id = task.clone();
    let mut pending = tokio::spawn(async move {
        service
            .execute(
                json!({"command":"task.start","task":task_id}),
                owner(&claim),
            )
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending)
            .await
            .is_err()
    );
    let state = f
        .service
        .store()
        .snapshot()
        .await
        .unwrap()
        .task_result(&task)
        .unwrap();
    assert_eq!(state["phase"], "PLANNING");
    f.clock.fetch_add(901, Ordering::SeqCst);
    drop(lock);
    assert!(pending.await.unwrap().is_err());
    let state = f
        .service
        .store()
        .snapshot()
        .await
        .unwrap()
        .task_result(&task)
        .unwrap();
    assert!(state["started_at"].is_null());
}

#[tokio::test]
async fn rejected_start_does_not_refresh_plan_metadata() {
    let f = Fixture::new("markdown").await;
    let task = f.task("unauthorized start").await;
    let claim = f.claim(&task, "owner").await;
    let plan = f.plan(&task).await;
    std::fs::write(
        plan["absolute_path"].as_str().unwrap(),
        "# Edited outside taskcli",
    )
    .unwrap();
    let before = f.service.store().snapshot().await.unwrap();
    let sequence = f.service.store().latest_sequence().await.unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"task.start","task":task}),
                WriteOptions {
                    session_ref: Some("other".into()),
                    ..owner(&claim)
                }
            )
            .await
            .is_err()
    );
    assert_eq!(f.service.store().snapshot().await.unwrap(), before);
    assert_eq!(f.service.store().latest_sequence().await.unwrap(), sequence);
}

#[tokio::test]
async fn board_and_job_show_planning_and_executing_without_extra_columns() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        let id = f.task("Visible phase").await;
        let claim = f.claim(&id, "visible").await;
        f.plan(&id).await;
        let state = f.service.store().snapshot().await.unwrap();
        let output = f.service.config().output_dir();
        let board = output.join(format!("Projects/{}/Board.md", state.projects[0].key));
        let job = output.join(&state.jobs[0].document_path);
        for phase in ["PLANNING", "EXECUTING"] {
            let body = std::fs::read_to_string(&board).unwrap();
            assert!(body.contains(phase), "{body}");
            assert!(body.contains(
                "| TODO | IN_PROGRESS | BLOCKED | WAITING_USER | DONE | FAILED | CANCELLED |"
            ));
            assert!(std::fs::read_to_string(&job).unwrap().contains(phase));
            if phase == "PLANNING" {
                f.service
                    .execute(json!({"command":"task.start","task":id}), owner(&claim))
                    .await
                    .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn planning_claim_excludes_other_writers_and_start_keeps_the_same_lease() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Plan after ownership").await;
    let plan = json!({"command":"plan.create","task":task,"body":"# Owned Plan"});
    assert!(
        f.service
            .execute(plan.clone(), WriteOptions::default())
            .await
            .is_err()
    );
    assert!(f.service.store().snapshot().await.unwrap().plans.is_empty());
    let claim = f.claim(&task, "owner").await;
    assert_eq!(claim["phase"], "PLANNING");
    assert!(claim["started_at"].is_null());
    for command in ["task.start", "task.done"] {
        assert!(
            f.service
                .execute(json!({"command":command,"task":task}), owner(&claim))
                .await
                .is_err()
        );
    }
    let other = WriteOptions {
        session_ref: Some("other".into()),
        ..owner(&claim)
    };
    assert!(f.service.execute(plan.clone(), other).await.is_err());
    let created = f.service.execute(plan, owner(&claim)).await.unwrap().result;
    assert!(std::path::Path::new(created["absolute_path"].as_str().unwrap()).is_file());
    assert!(
        f.service
            .execute(json!({"command":"task.done","task":task}), owner(&claim))
            .await
            .is_err()
    );
    let started = f
        .service
        .execute(json!({"command":"task.start","task":task}), owner(&claim))
        .await
        .unwrap()
        .result;
    assert_eq!(started["phase"], "EXECUTING");
    assert_eq!(started["lease"]["token"], claim["lease"]["token"]);
    assert!(started["started_at"].is_number());
    assert!(
        f.service
            .execute(json!({"command":"task.start","task":task}), owner(&claim))
            .await
            .is_err()
    );
    let done = f
        .service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap()
        .result;
    assert_eq!(done["status"], "DONE");
    assert!(done["phase"].is_null() && done["lease"].is_null());
}

#[tokio::test]
async fn planning_is_allowed_before_dependencies_finish_but_execution_is_not() {
    let f = Fixture::new("markdown").await;
    let dependency = f.task("Prerequisite").await;
    let task = f.task("Dependent planning").await;
    f.service
        .execute(
            json!({"command":"task.depend","task":task,"dependency":dependency}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let claim = f.claim(&task, "planner").await;
    f.service
        .execute(
            json!({"command":"plan.create","task":task,"body":"# Plan"}),
            owner(&claim),
        )
        .await
        .unwrap();
    assert!(
        f.service
            .execute(json!({"command":"task.start","task":task}), owner(&claim))
            .await
            .unwrap_err()
            .to_string()
            .contains("dependencies")
    );
    let upstream = f.claim(&dependency, "upstream").await;
    f.service
        .execute(
            json!({"command":"plan.create","task":dependency,"body":"# Upstream"}),
            owner(&upstream),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"task.start","task":dependency}),
            owner(&upstream),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"task.done","task":dependency}),
            owner(&upstream),
        )
        .await
        .unwrap();
    let started = f
        .service
        .execute(json!({"command":"task.start","task":task}), owner(&claim))
        .await
        .unwrap();
    assert_eq!(started.result["lease"]["token"], claim["lease"]["token"]);
}

#[tokio::test]
async fn a_planning_lease_recovers_without_a_plan_and_fences_the_old_owner() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Interrupted planning").await;
    let claim = f.claim(&task, "planner").await;
    f.service
        .execute(
            json!({"command":"session.end","session":"planner"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"session.start","session":"planner"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    let resumed = state.task_result(&task).unwrap();
    assert_eq!(resumed["phase"], "PLANNING");
    assert_ne!(resumed["lease"]["token"], claim["lease"]["token"]);
    let plan = json!({"command":"plan.create","task":task,"body":"# Recovered"});
    assert!(
        f.service
            .execute(plan.clone(), owner(&claim))
            .await
            .is_err()
    );
    f.service.execute(plan, owner(&resumed)).await.unwrap();
    f.clock.fetch_add(901, Ordering::SeqCst);
    assert!(
        f.service
            .execute(
                json!({"command":"plan.revise","task":task,"body":"# Stale"}),
                owner(&resumed)
            )
            .await
            .is_err()
    );
    assert_eq!(f.service.store().snapshot().await.unwrap().plans.len(), 1);
}

#[tokio::test]
async fn start_rejects_a_missing_or_empty_plan_file_without_consuming_the_lease() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Plan validation").await;
    let claim = f.claim(&task, "planner").await;
    let plan = f
        .service
        .execute(
            json!({"command":"plan.create","task":task,"body":"# Plan"}),
            owner(&claim),
        )
        .await
        .unwrap()
        .result;
    let path = std::path::Path::new(plan["absolute_path"].as_str().unwrap());
    std::fs::remove_file(path).unwrap();
    for content in [None, Some("   \n\t")] {
        if let Some(body) = content {
            std::fs::write(path, body).unwrap();
        }
        let before = f.service.store().snapshot().await.unwrap();
        assert!(
            f.service
                .execute(json!({"command":"task.start","task":task}), owner(&claim))
                .await
                .is_err()
        );
        assert_eq!(f.service.store().snapshot().await.unwrap(), before);
    }
    std::fs::write(path, "# Valid again").unwrap();
    f.service
        .execute(json!({"command":"task.start","task":task}), owner(&claim))
        .await
        .unwrap();
}

#[tokio::test]
async fn legacy_executing_tasks_migrate_without_losing_their_lease_or_history() {
    use sqlx::Connection;
    use sqlx::sqlite::SqliteConnectOptions;
    let f = Fixture::new("markdown").await;
    let task = f.task("Legacy execution").await;
    let path = &f.service.config().storage.path;
    let mut db = sqlx::SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET data = json_remove(json_set(data, '$.status', 'IN_PROGRESS', '$.started_at', 123, '$.last_session', 'legacy', '$.last_executor', 'agent:legacy'), '$.phase') WHERE id = ?").bind(&task).execute(&mut db).await.unwrap();
    let lease = json!({"task_id":task,"executor_ref":"agent:legacy","session_ref":"legacy","token":"legacy-token","delegated_by":null,"lease_expires_at":f.clock.load(Ordering::SeqCst)+900});
    sqlx::query("INSERT INTO task_leases(id,data) VALUES (?,?)")
        .bind(&task)
        .bind(lease.to_string())
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version = 1")
        .execute(&mut db)
        .await
        .unwrap();
    let migrated = Store::open_with_clock(path, {
        let clock = f.clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    })
    .await
    .unwrap();
    let result = migrated
        .snapshot()
        .await
        .unwrap()
        .task_result(&task)
        .unwrap();
    assert_eq!(result["phase"], "EXECUTING");
    assert_eq!(result["started_at"], 123);
    assert_eq!(result["lease"]["token"], "legacy-token");
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(version, 2);
}

#[tokio::test]
async fn starts_require_dependencies_and_claims_require_exclusive_ownership() {
    let f = Fixture::new("markdown").await;
    let a = f.task("database").await;
    let b = f.task("client").await;
    let cmd = json!({"command":"task.claim","task":a,"executor":"agent:a","session":"a"});
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
    let claim = f
        .service
        .execute(cmd, WriteOptions::default())
        .await
        .unwrap()
        .result;
    f.plan(&a).await;
    f.service
        .execute(json!({"command":"task.start","task":a}), owner(&claim))
        .await
        .unwrap();
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
    f.start(&b, "b").await;
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
    let old = f.start(&task, "old").await;
    f.clock.fetch_add(3600, Ordering::SeqCst);
    f.service.store().reap_expired().await.unwrap();
    assert_eq!(
        f.service.store().snapshot().await.unwrap().tasks[0]
            .status
            .to_string(),
        "BLOCKED"
    );
    let new = f.start(&task, "new").await;
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
    f.start(&id, "codex:one").await;
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
        state.tasks[0].phase,
        Some(agentix_task::TaskPhase::Planning)
    );
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
    let claim = f.start(&id, "s").await;
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
        let claim = f.claim(&id, "archive").await;
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
                owner(&claim),
            )
            .await
            .unwrap();
        assert_eq!(revised.result["version"], 2);
        assert_eq!(
            std::fs::read_to_string(&plan_path).unwrap(),
            "# My revised plan\n"
        );
        assert!(!before.is_empty());
        f.service
            .execute(json!({"command":"task.start","task":id}), owner(&claim))
            .await
            .unwrap();
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
    f.claim(&a, "a").await;
    f.claim(&b, "b").await;
    f.plan(&a).await;
    f.plan(&b).await;
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
    f.claim(&task, "links").await;
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
async fn special_obsidian_titles_keep_entities_outside_wikilink_aliases() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("渲染 | 中文 [链接] & <tag> [[injection]]").await;
    f.claim(&task, "special").await;
    f.plan(&task).await;
    let state = f.service.store().snapshot().await.unwrap();
    let board = std::fs::read_to_string(
        f.service
            .config()
            .output_dir()
            .join(format!("Projects/{}/Board.md", state.projects[0].key)),
    )
    .unwrap();
    assert!(
        board.contains("\\|Open]] 渲染 &#124; 中文 &#91;链接&#93; &amp; &lt;tag&gt; &#91;&#91;injection&#93;&#93;"),
        "{board}"
    );
    assert!(!board.contains("[[injection]]"));
}

#[tokio::test]
async fn plan_idempotent_replay_returns_the_same_result_and_path() {
    let f = Fixture::new("markdown").await;
    let task = f.task("idempotent plan").await;
    let claim = f.claim(&task, "idempotent").await;
    let request = json!({"command":"plan.create","task":task,"body":"# Plan"});
    let options = WriteOptions {
        idempotency_key: Some("plan-once".into()),
        ..owner(&claim)
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
    let claim = f.start(&task, "expired").await;
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
async fn missing_plan_allows_planning_resume_but_prevents_execution() {
    let f = Fixture::new("markdown").await;
    let task = f.task("missing plan").await;
    let claim = f.claim(&task, "missing").await;
    let plan = f.plan(&task).await;
    f.service
        .execute(json!({"command":"task.start","task":task}), owner(&claim))
        .await
        .unwrap();
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
    f.service
        .execute(
            json!({"command":"session.start","session":"missing"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let resumed = f
        .service
        .store()
        .snapshot()
        .await
        .unwrap()
        .task_result(&task)
        .unwrap();
    assert_eq!(resumed["phase"], "PLANNING");
    assert!(
        f.service
            .execute(json!({"command":"task.start","task":task}), owner(&resumed))
            .await
            .is_err()
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
    let job_path = f
        .service
        .config()
        .output_dir()
        .join(&f.service.store().snapshot().await.unwrap().jobs[0].document_path);
    let initial = std::fs::read_to_string(&job_path).unwrap().replace(
        "<!-- taskcli:notes:start -->",
        "<!-- taskcli:notes:start -->\nConcurrent notes survive.",
    );
    std::fs::write(&job_path, initial).unwrap();
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
    assert_eq!(body.matches("Concurrent notes survive.").count(), 1);
}

async fn task_in_state(f: &Fixture, status: &str) -> String {
    let id = f.task("state matrix").await;
    f.task("keep Job active").await;
    match status {
        "TODO" => {}
        "PLANNING" => {
            f.claim(&id, "matrix").await;
            f.plan(&id).await;
        }
        "IN_PROGRESS" | "DONE" | "FAILED" => {
            let claim = f.start(&id, "matrix").await;
            if status != "IN_PROGRESS" {
                let command = if status == "DONE" {
                    "task.done"
                } else {
                    "task.fail"
                };
                f.service
                    .execute(
                        json!({"command":command,"task":id,"reason":"matrix setup"}),
                        owner(&claim),
                    )
                    .await
                    .unwrap();
            }
        }
        _ => {
            let command = match status {
                "BLOCKED" => "task.block",
                "WAITING_USER" => "task.wait",
                "CANCELLED" => "task.cancel",
                _ => panic!("unknown fixture state"),
            };
            f.service
                .execute(
                    json!({"command":command,"task":id,"reason":"matrix setup"}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
        }
    }
    id
}

#[tokio::test]
async fn every_task_state_accepts_only_its_documented_commands_without_partial_writes() {
    let cases: [(&str, &[&str]); 8] = [
        ("TODO", &["claim", "block", "wait", "cancel"]),
        (
            "PLANNING",
            &["start", "block", "wait", "fail", "cancel", "heartbeat"],
        ),
        (
            "IN_PROGRESS",
            &["block", "wait", "done", "fail", "cancel", "heartbeat"],
        ),
        ("BLOCKED", &["claim", "wait", "fail", "cancel"]),
        ("WAITING_USER", &["claim", "block", "fail", "cancel"]),
        ("DONE", &["reopen"]),
        ("FAILED", &["retry"]),
        ("CANCELLED", &["reopen"]),
    ];
    for (status, allowed) in cases {
        for command in [
            "claim",
            "start",
            "block",
            "wait",
            "done",
            "fail",
            "cancel",
            "heartbeat",
            "retry",
            "reopen",
        ] {
            let f = Fixture::new("markdown").await;
            let id = task_in_state(&f, status).await;
            let before = f.service.store().snapshot().await.unwrap();
            let sequence = f.service.store().latest_sequence().await.unwrap();
            let options = if matches!(status, "IN_PROGRESS" | "PLANNING") {
                owner(&before.task_result(&id).unwrap())
            } else {
                WriteOptions::default()
            };
            let result = f.service.store().execute(json!({"command":format!("task.{command}"),"task":id,"reason":"matrix transition","executor":"agent:matrix","session":"matrix"}),options).await;
            assert_eq!(
                result.is_ok(),
                allowed.contains(&command),
                "{status} / {command}: {result:?}"
            );
            let after = f.service.store().snapshot().await.unwrap();
            if allowed.contains(&command) {
                let expected = match command {
                    "claim" | "start" | "heartbeat" => "IN_PROGRESS",
                    "block" => "BLOCKED",
                    "wait" => "WAITING_USER",
                    "done" => "DONE",
                    "fail" => "FAILED",
                    "cancel" => "CANCELLED",
                    _ => "TODO",
                };
                assert_eq!(after.task_result(&id).unwrap()["status"], expected);
                assert_eq!(after.leases.len(), usize::from(expected == "IN_PROGRESS"));
            } else {
                assert_eq!(
                    serde_json::to_value(before).unwrap(),
                    serde_json::to_value(after).unwrap()
                );
                assert_eq!(f.service.store().latest_sequence().await.unwrap(), sequence);
            }
        }
    }
}

#[tokio::test]
async fn same_executor_session_cannot_claim_two_ready_tasks_and_heartbeat_extends_lease() {
    let f = Fixture::new("markdown").await;
    let a = f.task("a").await;
    let b = f.task("b").await;
    let claim = f.claim(&a, "one").await;
    assert!(
        f.service
            .execute(
                json!({"command":"task.claim","task":b,"executor":"agent:one","session":"one"}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    f.clock.fetch_add(600, Ordering::SeqCst);
    f.service
        .execute(json!({"command":"task.heartbeat","task":a}), owner(&claim))
        .await
        .unwrap();
    f.clock.fetch_add(600, Ordering::SeqCst);
    assert_eq!(f.service.store().reap_expired().await.unwrap(), 0);
    f.service
        .execute(
            json!({"command":"task.release","task":a,"reason":"handoff"}),
            owner(&claim),
        )
        .await
        .unwrap();
    let b_claim = f.start(&b, "one").await;
    assert!(
        f.service
            .execute(json!({"command":"task.done","task":a}), owner(&claim))
            .await
            .is_err()
    );
    f.service
        .execute(json!({"command":"task.done","task":b}), owner(&b_claim))
        .await
        .unwrap();
}

#[tokio::test]
async fn plan_revision_requires_current_owner_and_rejected_writes_leave_no_file() {
    let f = Fixture::new("markdown").await;
    let id = f.task("owned plan").await;
    let claim = f.claim(&id, "owner").await;
    let plan = f.plan(&id).await;
    let request = json!({"command":"plan.revise","task":id,"body":"# New plan"});
    for options in [
        WriteOptions::default(),
        WriteOptions {
            session_ref: Some("wrong".into()),
            ..owner(&claim)
        },
        WriteOptions {
            expected_revision: Some(0),
            ..owner(&claim)
        },
    ] {
        assert!(f.service.execute(request.clone(), options).await.is_err());
        assert_eq!(f.service.store().snapshot().await.unwrap().plans.len(), 1);
        assert!(
            !std::path::Path::new(plan["absolute_path"].as_str().unwrap())
                .with_file_name("v002.md")
                .exists()
        );
    }
    let updated = f.service.execute(request, owner(&claim)).await.unwrap();
    assert_eq!(updated.result["version"], 2);
}

#[tokio::test]
async fn missing_or_duplicate_editable_markers_fail_without_overwriting_notes() {
    for duplicate in [false, true] {
        let f = Fixture::new("markdown").await;
        let state = f.service.store().snapshot().await.unwrap();
        let path = f
            .service
            .config()
            .output_dir()
            .join(&state.jobs[0].document_path);
        let body = std::fs::read_to_string(&path).unwrap().replace(
            "<!-- taskcli:notes:start -->",
            if duplicate {
                "<!-- taskcli:notes:start -->\n<!-- taskcli:notes:start -->\nKeep me"
            } else {
                "Keep me"
            },
        );
        std::fs::write(&path, &body).unwrap();
        assert!(f.service.sync().await.is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), body);
    }
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
    let claim = f.start(&task, "workflow").await;
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
    let claim = f.start(&task, "workflow").await;
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
    let claim = f.start(&task, "archive-recovery").await;
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

#[tokio::test]
async fn dependencies_can_cross_jobs_but_cannot_change_after_execution_starts() {
    let f = Fixture::new("markdown").await;
    let a = f.task("upstream").await;
    let job = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Downstream requirement"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let b = f
        .service
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"downstream"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.service
        .execute(
            json!({"command":"task.depend","task":b,"dependency":a}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let first = f.start(&a, "first").await;
    f.service
        .execute(json!({"command":"task.done","task":a}), owner(&first))
        .await
        .unwrap();
    let second = f.start(&b, "second").await;
    for command in ["task.depend", "task.undepend"] {
        assert!(
            f.service
                .execute(
                    json!({"command":command,"task":b,"dependency":a}),
                    owner(&second)
                )
                .await
                .is_err()
        );
    }
    f.service
        .execute(json!({"command":"task.done","task":b}), owner(&second))
        .await
        .unwrap();
    assert!(
        f.service
            .store()
            .snapshot()
            .await
            .unwrap()
            .jobs
            .iter()
            .all(|j| j.status == agentix_task::JobStatus::Completed)
    );
}

#[tokio::test]
async fn newer_database_schema_is_rejected_without_changing_its_version() {
    let f = Fixture::new("markdown").await;
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(&f.service.config().storage.path),
        )
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version = 99")
        .execute(&pool)
        .await
        .unwrap();
    assert!(Store::open(&f.service.config().storage.path).await.is_err());
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 99);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_redirect_document_output_or_managed_files_outside_the_root() {
    let f = Fixture::new("markdown").await;
    let outside = f.dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = f.service.config().documents.root.join("escape");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let mut config = f.service.config().clone();
    config.documents.directory = "escape".into();
    assert!(config.validate().is_err());
    let target = outside.join("Dashboard.md");
    std::fs::write(&target, "Outside content must survive").unwrap();
    let board = f.service.config().output_dir().join("Dashboard.md");
    std::fs::rename(&board, f.dir.path().join("original-dashboard.md")).unwrap();
    std::os::unix::fs::symlink(&target, &board).unwrap();
    assert!(f.service.sync().await.is_err());
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "Outside content must survive"
    );
}
