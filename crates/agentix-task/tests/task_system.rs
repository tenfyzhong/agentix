use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use agentix_task::{Config, Service, Store, WriteOptions};
use serde_json::{Value, json};
use tempfile::TempDir;

#[path = "support/deletion.rs"]
mod deletion;

#[path = "support/tasknotes.rs"]
mod tasknotes;

#[path = "support/job_graph.rs"]
mod job_graph;

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
        std::fs::create_dir_all(&root).unwrap();
        if format == "obsidian" {
            std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        }
        let config: Config = toml::from_str(&format!(
            "schema_version = 1\n[storage]\npath = {:?}\n[documents]\nformat = {format:?}\nroot = {:?}\ndirectory = 'Tasks \u{2603}'\n",
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

fn task_status_names() -> Vec<String> {
    agentix_task::TaskStatus::ALL
        .iter()
        .map(ToString::to_string)
        .collect()
}

async fn populate_board_states(f: &Fixture) {
    for (title, command) in [
        ("TODO", None),
        ("IN_PROGRESS", Some("task.claim")),
        ("BLOCKED", Some("task.block")),
        ("WAITING_USER", Some("task.wait")),
        ("DONE", Some("task.done")),
        ("FAILED", Some("task.fail")),
        ("CANCELLED", Some("task.cancel")),
    ] {
        let task = f.task(title).await;
        match command {
            Some("task.claim") => {
                f.claim(&task, title).await;
            }
            Some("task.done") => {
                let claim = f.start(&task, title).await;
                f.service
                    .execute(json!({"command":"task.done","task":task}), owner(&claim))
                    .await
                    .unwrap();
            }
            Some(command) => {
                let options = if command == "task.fail" {
                    owner(&f.claim(&task, title).await)
                } else {
                    WriteOptions::default()
                };
                f.service
                    .execute(
                        json!({"command":command,"task":task,"reason":"View coverage"}),
                        options,
                    )
                    .await
                    .unwrap();
            }
            None => {}
        }
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
async fn task_note_shows_planning_and_executing_in_frontmatter() {
    let f = Fixture::new("obsidian").await;
    let id = f.task("Visible phase").await;
    let claim = f.claim(&id, "visible").await;
    let plan = f.plan(&id).await;
    for phase in ["PLANNING", "EXECUTING"] {
        let body = std::fs::read_to_string(plan["absolute_path"].as_str().unwrap()).unwrap();
        assert!(body.contains(&format!("phase: \"{phase}\"")));
        if phase == "PLANNING" {
            f.service
                .execute(json!({"command":"task.start","task":id}), owner(&claim))
                .await
                .unwrap();
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
    assert_eq!(version, 7);
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
        let id = f.task("Task | Unicode \u{2603} [x]").await;
        let claim = f.claim(&id, "archive").await;
        let plan = f.plan(&id).await;
        let state = f.service.store().snapshot().await.unwrap();
        let project = &state.projects[0];
        let output = f.service.config().output_dir();
        let job_path = output.join(&state.jobs[0].document_path);
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
        assert!(board.contains("tasknotesKanban"));
        assert_eq!(board.contains("[["), format == "obsidian");
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
        assert!(
            std::fs::read_to_string(&plan_path)
                .unwrap()
                .ends_with("# Second version\n")
        );
        assert_eq!(revised.result["absolute_path"], plan["absolute_path"]);
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
        assert!(archived.contains("/Jobs/Archived/"));
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
async fn dashboard_stays_project_only_as_jobs_grow() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        let output = f.service.config().output_dir();
        let before = std::fs::read_to_string(output.join("Dashboard.md")).unwrap();
        for index in 0..20 {
            f.service
                .store()
                .execute(
                    json!({"command":"job.create", "project":f.project,
                        "title":format!("Extra job {index}"), "goal":"Ship it"}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
        }
        f.service.sync().await.unwrap();
        let dashboard = std::fs::read_to_string(output.join("Dashboard.md")).unwrap();
        assert_eq!(
            dashboard, before,
            "Job count must not change {format} Dashboard"
        );
        assert!(dashboard.contains("## demo"));
        for entry in ["meta", "Board"] {
            assert!(dashboard.contains(&format!("Projects/demo/{entry}")));
        }
        assert!(!dashboard.contains("Feature"));
        assert!(!dashboard.contains("Jobs/"));
        for job in f.service.store().snapshot().await.unwrap().jobs {
            assert!(output.join(job.document_path).is_file());
        }
    }
}

#[tokio::test]
async fn job_type_tags_switch_exclusively_on_archive_and_restore() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        for (command, tag) in [
            ("job.cancel", "agent/job"),
            ("job.archive", "agent/archived/job"),
            ("job.unarchive", "agent/job"),
        ] {
            let job = f
                .service
                .execute(
                    json!({"command":command,"job":f.job}),
                    WriteOptions::default(),
                )
                .await
                .unwrap()
                .result;
            let path = f
                .service
                .config()
                .output_dir()
                .join(job["document_path"].as_str().unwrap());
            for repair in [false, true] {
                if repair {
                    let old = std::fs::read_to_string(&path).unwrap();
                    let old = old
                        .lines()
                        .map(|line| {
                            if line.starts_with("tags:") {
                                "tags: [agent/job, agent/archived/job]"
                            } else {
                                line
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    std::fs::write(&path, old).unwrap();
                    f.service.sync().await.unwrap();
                }
                let doc = std::fs::read_to_string(&path).unwrap();
                let yaml = doc
                    .strip_prefix("---\n")
                    .unwrap()
                    .split_once("\n---\n")
                    .unwrap()
                    .0;
                let properties: Value = serde_yaml::from_str(yaml).unwrap();
                assert_eq!(properties["tags"], json!([tag]), "{format}: {command}");
            }
        }
    }
}

#[tokio::test]
async fn job_frontmatter_omits_paths_titles_names_and_embedded_tasks() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        f.task("Visible task").await;
        for archived in [false, true] {
            if archived {
                for command in ["job.cancel", "job.archive"] {
                    f.service
                        .execute(
                            json!({"command":command,"job":f.job}),
                            WriteOptions::default(),
                        )
                        .await
                        .unwrap();
                }
            }
            let state = f.service.store().snapshot().await.unwrap();
            let job = &state.jobs[0];
            let doc =
                std::fs::read_to_string(f.service.config().output_dir().join(&job.document_path))
                    .unwrap();
            let (_, rest) = doc.split_once("---\n").unwrap();
            let (yaml, body) = rest.split_once("---\n").unwrap();
            let properties: Value = serde_yaml::from_str(yaml).unwrap();
            for field in ["document_path", "task", "tasks", "title", "name"] {
                assert!(
                    properties.get(field).is_none(),
                    "{field} must be absent in {format} Job properties"
                );
            }
            for field in [
                "id",
                "project_id",
                "status",
                "revision",
                "sequence",
                "created_at",
                "updated_at",
                "started_at",
                "completed_at",
                "cancelled_at",
                "archived_at",
                "tags",
            ] {
                assert!(properties.get(field).is_some(), "{field} must remain");
            }
            assert_eq!(properties["id"], f.job);
            assert_eq!(properties["archived_at"].is_null(), !archived);
            assert!(body.contains("# Feature"));
            assert!(body.contains("Visible task"));
            assert_eq!(job.name, "Feature");
            assert_eq!(job.title, "Feature");
            assert_eq!(state.tasks.len(), 1);
        }
    }
}

#[tokio::test]
async fn job_tasks_reference_notes_and_sync_removes_legacy_checkboxes() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        f.task("Tagged task").await;
        for archived in [false, true] {
            if archived {
                for command in ["job.cancel", "job.archive"] {
                    f.service
                        .execute(
                            json!({"command":command,"job":f.job}),
                            WriteOptions::default(),
                        )
                        .await
                        .unwrap();
                }
            }
            let state = f.service.store().snapshot().await.unwrap();
            let path = f
                .service
                .config()
                .output_dir()
                .join(&state.jobs[0].document_path);
            // Simulate a Job generated by an older taskcli installation.
            let old = std::fs::read_to_string(&path)
                .unwrap()
                .replace("- [[", "- [ ] #task [[")
                .replace("- [260", "- [ ] #task [260");
            std::fs::write(&path, old).unwrap();
            f.service.sync().await.unwrap();
            let document = std::fs::read_to_string(&path).unwrap();
            let tasks: Vec<_> = document
                .lines()
                .filter(|line| line.starts_with("- ["))
                .collect();
            assert_eq!(tasks.len(), 1);
            assert!(!tasks[0].contains("#task"));
            assert!(!tasks[0].contains("#agent/task"));
            assert!(tasks[0].contains("Tasks/"));
        }
    }
}

#[tokio::test]
async fn job_task_references_display_plan_filenames_after_rename_and_archive() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        let task = f.task("迁移语言配置").await;
        let claim = f.claim(&task, "filename-label").await;
        f.plan(&task).await;
        for step in 0..3 {
            if step == 1 {
                f.service
                    .execute(
                        json!({"command":"task.update","task":task,"name":"更新语言配置"}),
                        owner(&claim),
                    )
                    .await
                    .unwrap();
            } else if step == 2 {
                f.service
                    .execute(
                        json!({"command":"task.release","task":task,"reason":"Archive fixture"}),
                        owner(&claim),
                    )
                    .await
                    .unwrap();
                for command in ["job.cancel", "job.archive"] {
                    f.service
                        .execute(json!({"command":command,"job":f.job}), owner(&claim))
                        .await
                        .unwrap();
                }
            }
            let state = f.service.store().snapshot().await.unwrap();
            let plan = &state.plans[0];
            let filename = std::path::Path::new(&plan.path)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap();
            let document = std::fs::read_to_string(
                f.service
                    .config()
                    .output_dir()
                    .join(&state.jobs[0].document_path),
            )
            .unwrap();
            let expected = if format == "obsidian" {
                format!(
                    "[[Tasks ☃/{}|{filename}]]",
                    plan.path.trim_end_matches(".md")
                )
            } else {
                format!("[{filename}](")
            };
            assert!(
                document.contains(&expected),
                "{format}, step {step}: {document}"
            );
            assert!(!document.contains("|Plan]]"));
            assert!(!document.contains("![Plan]("));
        }
    }
}

#[tokio::test]
async fn obsidian_alias_separators_are_not_table_escaped_in_task_links() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Linked task").await;
    f.claim(&task, "links").await;
    f.plan(&task).await;
    let state = f.service.store().snapshot().await.unwrap();
    let output = f.service.config().output_dir();
    let dashboard = std::fs::read_to_string(output.join("Dashboard.md")).unwrap();
    assert!(dashboard.contains("|Kanban board]]"));
    assert!(!dashboard.contains("\\|Kanban board]]"));
    let job = std::fs::read_to_string(output.join(&state.jobs[0].document_path)).unwrap();
    assert!(job.contains("|260905-0001-Linked task]]"));
    assert!(!job.contains("\\|260905-0001-Linked task]]"));
}

#[tokio::test]
async fn special_obsidian_titles_keep_entities_outside_wikilink_aliases() {
    let f = Fixture::new("obsidian").await;
    let task = f
        .task("Render | Unicode \u{2603} [link] & <tag> [[injection]]")
        .await;
    f.claim(&task, "special").await;
    f.plan(&task).await;
    let state = f.service.store().snapshot().await.unwrap();
    let board = std::fs::read_to_string(
        f.service
            .config()
            .output_dir()
            .join(&state.jobs[0].document_path),
    )
    .unwrap();
    assert!(
        board.contains("Render Unicode \u{2603} link &amp; tag injection")
            && board.contains("|Open]]"),
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
        if format == "markdown" {
            assert!(body.contains(&format!("<a id=\"{}\"></a>", first.replace('_', "-"))));
        }
        let dependencies = body
            .lines()
            .find(|line| line.trim_start().starts_with("Dependencies:"))
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

#[tokio::test]
async fn numbered_filenames_use_creation_day_and_project_scoped_daily_sequences() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("First task").await;
    for index in 2..=11 {
        let job = f.service.execute(
            json!({"command":"job.create", "project":f.project, "title":format!("Job {index}")}),
            WriteOptions::default(),
        ).await.unwrap().result;
        assert_eq!(job["sequence"], index);
        assert_eq!(
            job["document_path"],
            format!("Projects/demo/Jobs/260905-{index:04}-Job {index}.md")
        );
        let task = f
            .service
            .execute(
                json!({"command":"task.add", "job":job["id"], "title":format!("Task {index}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        assert_eq!(task["sequence"], index);
    }
    let state = f.service.store().snapshot().await.unwrap();
    let paths: Vec<_> = state.jobs.iter().map(|j| j.document_path.clone()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    f.clock.fetch_add(86400, Ordering::SeqCst);
    f.claim(&task, "next-day").await;
    assert_eq!(
        f.plan(&task).await["path"],
        "Projects/demo/Tasks/260905-0001-First task.md"
    );
    let next = f
        .service
        .execute(
            json!({"command":"job.create", "project":f.project, "title":"Next day"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(next["sequence"], 1);
    assert_eq!(
        next["document_path"],
        "Projects/demo/Jobs/260906-0001-Next day.md"
    );
    let next_task = f.task("Next task").await;
    f.claim(&next_task, "another").await;
    assert_eq!(
        f.plan(&next_task).await["path"],
        "Projects/demo/Tasks/260906-0001-Next task.md"
    );
    let other_project = f.service.execute(
        json!({"command":"project.register", "name":"other", "root":f.dir.path().join("other")}),
        WriteOptions::default(),
    ).await.unwrap().result;
    let other_job = f
        .service
        .execute(
            json!({"command":"job.create", "project":other_project["id"], "title":"Independent"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(other_job["sequence"], 1);
    assert_eq!(
        other_job["document_path"],
        "Projects/other/Jobs/260906-0001-Independent.md"
    );
}

#[tokio::test]
async fn numbered_filenames_are_allocated_atomically_and_survive_archive() {
    let f = Fixture::new("markdown").await;
    let now = f.clock.clone();
    let other = Store::open_with_clock(
        &f.service.config().storage.path,
        Arc::new(move || now.load(Ordering::SeqCst)),
    )
    .await
    .unwrap();
    let request = json!({"command":"job.create", "project":f.project, "title":"Concurrent"});
    let (a, b) = tokio::join!(
        f.service
            .store()
            .execute(request.clone(), WriteOptions::default()),
        other.execute(request, WriteOptions::default()),
    );
    let a = a.unwrap().result;
    let b = b.unwrap().result;
    let mut sequences = vec![
        a["sequence"].as_u64().unwrap(),
        b["sequence"].as_u64().unwrap(),
    ];
    sequences.sort_unstable();
    assert_eq!(sequences, [2, 3]);
    let request = json!({"command":"task.add", "job":f.job, "title":"Concurrent task"});
    let (a, b) = tokio::join!(
        f.service
            .store()
            .execute(request.clone(), WriteOptions::default()),
        other.execute(request, WriteOptions::default()),
    );
    let mut sequences = vec![
        a.unwrap().result["sequence"].as_u64().unwrap(),
        b.unwrap().result["sequence"].as_u64().unwrap(),
    ];
    sequences.sort_unstable();
    assert_eq!(sequences, [1, 2]);
    f.service
        .execute(
            json!({"command":"job.cancel", "job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.clock.fetch_add(86400, Ordering::SeqCst);
    let archived = f
        .service
        .execute(
            json!({"command":"job.archive", "job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(
        archived["document_path"],
        "Projects/demo/Jobs/Archived/260905-0001-Feature.md"
    );
    let restored = f
        .service
        .execute(
            json!({"command":"job.unarchive", "job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(
        restored["document_path"],
        "Projects/demo/Jobs/260905-0001-Feature.md"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Build legacy active/archive files and verify metadata, content, and links in both formats.
async fn jobs_layout_migrates_v4_active_and_archived_documents() {
    use sqlx::Connection;
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        let archived = f
            .service
            .execute(
                json!({"command":"job.create", "project":f.project, "title":"Old work"}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        for command in ["job.cancel", "job.archive"] {
            f.service
                .execute(
                    json!({"command":command,"job":archived["id"]}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
        }
        f.task("Linked task").await;
        let before = f.service.store().snapshot().await.unwrap();
        let root = f.service.config().output_dir();
        let mut db = sqlx::SqliteConnection::connect(&format!(
            "sqlite:{}",
            f.service.config().storage.path.display()
        ))
        .await
        .unwrap();
        let mut documents = f
            .service
            .store()
            .metadata("documents")
            .await
            .unwrap()
            .unwrap();
        for job in &before.jobs {
            let folder = if job.archived_at.is_some() {
                "Archive/2026/09"
            } else {
                "Active"
            };
            let filename = job.document_path.rsplit('/').next().unwrap();
            let legacy = format!("Projects/demo/Jobs/{folder}/{filename}");
            let body = std::fs::read_to_string(root.join(&job.document_path))
                .unwrap()
                .replace(
                    "<!-- taskcli:notes:start -->",
                    "<!-- taskcli:notes:start -->\nPreserve notes.",
                );
            std::fs::remove_file(root.join(&job.document_path)).unwrap();
            std::fs::create_dir_all(root.join(&legacy).parent().unwrap()).unwrap();
            std::fs::write(root.join(&legacy), body).unwrap();
            sqlx::query("UPDATE jobs SET data = json_set(data, '$.document_path', ?) WHERE id = ?")
                .bind(&legacy)
                .bind(&job.id)
                .execute(&mut db)
                .await
                .unwrap();
            documents[format!("job:{}", job.id)] = json!(legacy);
        }
        f.service
            .store()
            .set_metadata("documents", &documents)
            .await
            .unwrap();
        sqlx::query("PRAGMA user_version = 4")
            .execute(&mut db)
            .await
            .unwrap();
        let service = Service::open(f.service.config().clone()).await.unwrap();
        service.sync().await.unwrap();
        let after = service.store().snapshot().await.unwrap();
        for (old, job) in before.jobs.iter().zip(&after.jobs) {
            let folder = if job.archived_at.is_some() {
                "Archived/"
            } else {
                ""
            };
            let filename = old.document_path.rsplit('/').next().unwrap();
            assert_eq!(
                job.document_path,
                format!("Projects/demo/Jobs/{folder}{filename}")
            );
            assert_eq!(
                (
                    job.sequence,
                    job.created_at,
                    job.archived_at,
                    job.status,
                    job.revision
                ),
                (
                    old.sequence,
                    old.created_at,
                    old.archived_at,
                    old.status,
                    old.revision
                )
            );
            let text = std::fs::read_to_string(root.join(&job.document_path)).unwrap();
            assert!(text.contains("Preserve notes."));
            assert_eq!(
                text.contains("agent/archived/job"),
                job.archived_at.is_some()
            );
        }
        assert!(!root.join("Projects/demo/Jobs/Active").exists());
        assert!(!root.join("Projects/demo/Jobs/Archive").exists());
        let board = std::fs::read_to_string(root.join("Projects/demo/Board.md")).unwrap();
        assert!(board.contains("tasknotesKanban"));
        assert!(!board.contains("Jobs/Active/"));
        service.sync().await.unwrap();
        assert_eq!(service.store().snapshot().await.unwrap().jobs, after.jobs);
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Exercise legacy files, failed migration publication, and restart recovery together.
async fn numbered_filenames_migrate_v3_and_recover_after_a_destination_conflict() {
    use sqlx::Connection;
    let f = Fixture::new("markdown").await;
    let task = f.task("Keep plan").await;
    let claim = f.claim(&task, "migration").await;
    f.plan(&task).await;
    let state = f.service.store().snapshot().await.unwrap();
    let root = f.service.config().output_dir();
    let old_job = "Projects/demo/Jobs/Active/Feature.md";
    let old_plan = "Projects/demo/Plans/Keep plan.md";
    let job_body = std::fs::read_to_string(root.join(&state.jobs[0].document_path))
        .unwrap()
        .replace(
            "<!-- taskcli:notes:start -->",
            "<!-- taskcli:notes:start -->\nKeep notes",
        );
    std::fs::remove_file(root.join(&state.jobs[0].document_path)).unwrap();
    std::fs::remove_file(root.join(&state.plans[0].path)).unwrap();
    std::fs::create_dir_all(root.join(old_job).parent().unwrap()).unwrap();
    std::fs::write(root.join(old_job), job_body).unwrap();
    std::fs::create_dir_all(root.join(old_plan).parent().unwrap()).unwrap();
    std::fs::write(
        root.join(old_plan),
        "---\ncustom: retained\n---\nKeep authored plan.",
    )
    .unwrap();
    let mut db = sqlx::SqliteConnection::connect(&format!(
        "sqlite:{}",
        f.service.config().storage.path.display()
    ))
    .await
    .unwrap();
    sqlx::query(
        "UPDATE jobs SET data = json_remove(json_set(data, '$.document_path', ?), '$.sequence')",
    )
    .bind(old_job)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query("UPDATE tasks SET data = json_remove(data, '$.sequence')")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE plans SET data = json_set(data, '$.path', ?)")
        .bind(old_plan)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM projection_state WHERE key = 'documents'")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version = 3")
        .execute(&mut db)
        .await
        .unwrap();
    let destination = root.join("Projects/demo/Tasks/260905-0001-Keep plan.md");
    std::fs::write(&destination, "Personal note").unwrap();
    let now = f.clock.clone();
    let store = Store::open_with_clock(
        &f.service.config().storage.path,
        Arc::new(move || now.load(Ordering::SeqCst)),
    )
    .await
    .unwrap();
    let service = Service::new(f.service.config().clone(), store).unwrap();
    assert!(service.sync().await.is_err());
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "Personal note"
    );
    assert!(root.join(old_job).exists());
    assert!(root.join(old_plan).exists());
    std::fs::remove_file(&destination).unwrap();
    drop(service);
    let now = f.clock.clone();
    let store = Store::open_with_clock(
        &f.service.config().storage.path,
        Arc::new(move || now.load(Ordering::SeqCst)),
    )
    .await
    .unwrap();
    let service = Service::new(f.service.config().clone(), store).unwrap();
    service.sync().await.unwrap();
    let migrated = service.store().snapshot().await.unwrap();
    assert_eq!(
        migrated.plans[0].path,
        "Projects/demo/Tasks/260905-0001-Keep plan.md"
    );
    assert_eq!(migrated.plans[0].id, state.plans[0].id);
    assert_eq!(migrated.plans[0].version, state.plans[0].version);
    assert_eq!(migrated.tasks[0].revision, state.tasks[0].revision);
    assert_eq!(migrated.leases, state.leases);
    let body = std::fs::read_to_string(&destination).unwrap();
    assert!(body.contains("Keep authored plan."));
    assert!(body.contains("custom: \"retained\""));
    assert!(
        std::fs::read_to_string(root.join(&migrated.jobs[0].document_path))
            .unwrap()
            .contains("Keep notes")
    );
    assert!(
        std::fs::read_to_string(
            root.join(&service.store().snapshot().await.unwrap().jobs[0].document_path)
        )
        .unwrap()
        .contains("260905-0001-Keep%20plan.md")
    );
    assert!(!root.join(old_job).exists());
    assert!(!root.join(old_plan).exists());
    service.sync().await.unwrap();
    assert_eq!(body, std::fs::read_to_string(destination).unwrap());
    service
        .execute(json!({"command":"task.start", "task":task}), owner(&claim))
        .await
        .unwrap();
}

#[tokio::test]
async fn readable_names_only_gain_suffixes_on_collision() {
    let f = Fixture::new("obsidian").await;
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.projects[0].key, "demo");
    assert_eq!(
        state.jobs[0].document_path,
        "Projects/demo/Jobs/260905-0001-Feature.md"
    );
    let duplicate = f
        .service
        .execute(
            json!({"command":"project.register","name":"demo","root":f.dir.path().join("other")}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(duplicate["key"], "demo-2");
    assert_eq!(duplicate["name"], "demo-2");
    let other = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Feature"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(
        other["document_path"],
        "Projects/demo/Jobs/260905-0002-Feature-2.md"
    );
    let a = f.task("实现功能").await;
    let b = f.task("实现功能").await;
    f.claim(&a, "a").await;
    f.claim(&b, "b").await;
    assert_eq!(
        f.plan(&a).await["path"],
        "Projects/demo/Tasks/260905-0001-实现功能.md"
    );
    assert_eq!(
        f.plan(&b).await["path"],
        "Projects/demo/Tasks/260905-0002-实现功能-2.md"
    );
}

#[tokio::test]
async fn plan_revisions_replace_one_file_and_keep_lifecycle_properties() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Short task").await;
    let claim = f.claim(&task, "writer").await;
    let first = f.plan(&task).await;
    f.clock.fetch_add(10, Ordering::SeqCst);
    let second = f
        .service
        .execute(
            json!({"command":"plan.revise","task":task,"body":"# Revised\nAcceptance."}),
            owner(&claim),
        )
        .await
        .unwrap();
    assert_eq!(second.result["path"], first["path"]);
    assert_eq!(second.result["id"], first["id"]);
    assert_eq!(second.result["version"], 2);
    assert_eq!(f.service.store().snapshot().await.unwrap().plans.len(), 1);
    let path = first["absolute_path"].as_str().unwrap();
    let contents = std::fs::read_to_string(path).unwrap();
    assert!(contents.starts_with("---\n"));
    assert!(contents.contains("agent/task"));
    assert!(!contents.lines().any(|line| line.starts_with("version:")));
    assert!(contents.contains("revision:"));
    assert!(contents.ends_with("# Revised\nAcceptance."));
    assert!(!contents.contains("Implement and verify"));
    assert_eq!(
        std::fs::read_dir(std::path::Path::new(path).parent().unwrap())
            .unwrap()
            .count(),
        1
    );
    f.service
        .execute(json!({"command":"task.start","task":task}), owner(&claim))
        .await
        .unwrap();
    f.clock.fetch_add(10, Ordering::SeqCst);
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let contents = std::fs::read_to_string(path).unwrap();
    for (field, seconds) in [
        ("started_at", 1_788_566_410),
        ("completed_at", 1_788_566_420),
    ] {
        let instant = time::OffsetDateTime::from_unix_timestamp(seconds).unwrap();
        let expected = instant
            .to_offset(time::UtcOffset::local_offset_at(instant).unwrap())
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        assert!(
            contents.contains(&format!("{field}: {expected:?}")),
            "{contents}"
        );
    }
}

#[tokio::test]
async fn metadata_and_status_checklists_include_completed_jobs_until_archived() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
        let task = f.task("Short task").await;
        let claim = f.start(&task, "complete").await;
        f.service
            .execute(json!({"command":"task.done","task":task}), owner(&claim))
            .await
            .unwrap();
        let state = f.service.store().snapshot().await.unwrap();
        let root = f.service.config().output_dir();
        let board = std::fs::read_to_string(
            root.join(format!("Projects/{}/Board.md", state.projects[0].key)),
        )
        .unwrap();
        assert!(board.contains("DONE"));
        let task_path = root.join(&state.plans[0].path);
        let task_doc = std::fs::read_to_string(&task_path).unwrap();
        assert!(task_doc.contains("status: \"DONE\""));
        assert!(task_doc.contains("archived: false"));
        for (path, tag) in [("meta.md", "agent/project"), ("Board.md", "agent/board")] {
            let body = std::fs::read_to_string(
                root.join(format!("Projects/{}/{path}", state.projects[0].key)),
            )
            .unwrap();
            let fm = body
                .strip_prefix("---\n")
                .unwrap()
                .split_once("\n---\n")
                .unwrap()
                .0;
            assert!(fm.contains(tag));
            assert!(fm.contains("id:"));
            assert!(fm.contains("created_at:"));
            if path == "meta.md" {
                assert!(fm.contains("sync_status:"));
                let properties: Value = serde_yaml::from_str(fm).unwrap();
                assert_eq!(properties["sync_status"], "synced");
                assert!(properties["sync_sequence"].is_number());
                assert!(fm.contains("root:"));
                assert!(fm.contains("remote:"));
            }
        }
        let path = root.join(&state.jobs[0].document_path);
        let body = std::fs::read_to_string(&path).unwrap();
        let (fm, body) = body
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap();
        for key in [
            "id:",
            "status:",
            "revision:",
            "created_at:",
            "started_at:",
            "completed_at:",
            "agent/job",
        ] {
            assert!(fm.contains(key), "{fm}");
        }
        assert!(!body.contains(&f.job));
        assert!(body.contains("260905-0001-Short task"), "{body}");
        assert!(body.contains(if format == "obsidian" { "[[" } else { "[" }));
        f.service
            .execute(
                json!({"command":"job.archive","job":f.job}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let job = &f.service.store().snapshot().await.unwrap().jobs[0];
        assert!(
            std::fs::read_to_string(root.join(&job.document_path))
                .unwrap()
                .contains("agent/archived/job")
        );
        assert!(
            std::fs::read_to_string(&task_path)
                .unwrap()
                .contains("archived: true")
        );
    }
}

#[tokio::test]
async fn project_archive_requires_closed_work_and_can_be_reversed() {
    let f = Fixture::new("markdown").await;
    let request = json!({"command":"project.archive","project":f.project});
    assert!(
        f.service
            .execute(request.clone(), WriteOptions::default())
            .await
            .is_err()
    );
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let archived = f
        .service
        .execute(request, WriteOptions::default())
        .await
        .unwrap();
    assert!(archived.result["archived_at"].is_number());
    let dashboard =
        std::fs::read_to_string(f.service.config().output_dir().join("Dashboard.md")).unwrap();
    assert!(!dashboard.contains("## demo"));
    assert!(
        f.service
            .execute(
                json!({"command":"job.create","project":f.project,"title":"New"}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    f.service
        .execute(
            json!({"command":"project.unarchive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let dashboard =
        std::fs::read_to_string(f.service.config().output_dir().join("Dashboard.md")).unwrap();
    assert!(dashboard.contains("## demo"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Arrange an actual legacy database and verify recovered files.
async fn legacy_layout_migrates_without_losing_notes_or_latest_plan() {
    use sqlx::Connection;
    let f = Fixture::new("markdown").await;
    let task = f.task("Read logs").await;
    f.claim(&task, "migration").await;
    f.plan(&task).await;
    let state = f.service.store().snapshot().await.unwrap();
    let root = f.service.config().output_dir();
    let legacy_root = "Projects/demo-12345678";
    let legacy_job = format!("{legacy_root}/Jobs/Active/{}.md", f.job);
    let legacy_plan = format!("{legacy_root}/Plans/{task}/v002.md");
    let legacy_first = format!("{legacy_root}/Plans/{task}/v001.md");
    let mut documents = f
        .service
        .store()
        .metadata("documents")
        .await
        .unwrap()
        .unwrap();
    for value in documents.as_object_mut().unwrap().values_mut() {
        if let Some(path) = value.as_str() {
            *value = json!(path.replace("Projects/demo/", &format!("{legacy_root}/")));
        }
    }
    documents[format!("job:{}", f.job)] = json!(legacy_job);
    documents[format!("plan:{}", state.plans[0].id)] = json!(legacy_plan);
    let job_body = std::fs::read_to_string(root.join(&state.jobs[0].document_path))
        .unwrap()
        .replace(
            "<!-- taskcli:notes:start -->",
            "<!-- taskcli:notes:start -->\nKeep my notes",
        );
    std::fs::create_dir_all(root.join(&legacy_job).parent().unwrap()).unwrap();
    std::fs::create_dir_all(root.join(&legacy_plan).parent().unwrap()).unwrap();
    std::fs::write(root.join(&legacy_job), job_body).unwrap();
    std::fs::write(
        root.join(&legacy_plan),
        "# Latest plan\nKeep acceptance checks.",
    )
    .unwrap();
    std::fs::write(root.join(&legacy_first), "# Obsolete version").unwrap();
    std::fs::remove_file(root.join(&state.jobs[0].document_path)).unwrap();
    std::fs::remove_file(root.join(&state.plans[0].path)).unwrap();
    let mut db = sqlx::SqliteConnection::connect(&format!(
        "sqlite:{}",
        f.service.config().storage.path.display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE projects SET data = json_set(data, '$.key', 'demo-12345678')")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET data = json_remove(json_set(data, '$.document_path', ?), '$.name', '$.started_at')").bind(&legacy_job).execute(&mut db).await.unwrap();
    sqlx::query("UPDATE tasks SET data = json_remove(data, '$.name', '$.completed_at')")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE plans SET data = json_remove(json_set(data, '$.path', ?, '$.version', 2), '$.updated_at', '$.pending_body')").bind(&legacy_plan).execute(&mut db).await.unwrap();
    let mut old = serde_json::to_value(&state.plans[0]).unwrap();
    old["id"] = json!("plan_legacy");
    old["version"] = json!(1);
    old["path"] = json!(legacy_first);
    sqlx::query("INSERT INTO plans(id,data) VALUES ('plan_legacy',?)")
        .bind(old.to_string())
        .execute(&mut db)
        .await
        .unwrap();
    f.service
        .store()
        .set_metadata("documents", &documents)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version = 2")
        .execute(&mut db)
        .await
        .unwrap();
    let service = Service::open(f.service.config().clone()).await.unwrap();
    service.sync().await.unwrap();
    let migrated = service.store().snapshot().await.unwrap();
    assert_eq!(migrated.projects[0].key, "demo");
    assert_eq!(migrated.plans.len(), 1);
    assert_eq!(
        migrated.plans[0].path,
        "Projects/demo/Tasks/260905-0001-Read logs.md"
    );
    assert!(
        std::fs::read_to_string(root.join(&migrated.plans[0].path))
            .unwrap()
            .contains("Keep acceptance checks.")
    );
    assert!(
        std::fs::read_to_string(root.join(&migrated.jobs[0].document_path))
            .unwrap()
            .contains("Keep my notes")
    );
    assert!(!root.join(legacy_first).exists());
    assert!(!root.join(legacy_plan).exists());
    assert!(!root.join(legacy_job).exists());
    let first = std::fs::read_to_string(root.join(&migrated.plans[0].path)).unwrap();
    service.sync().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join(&migrated.plans[0].path)).unwrap(),
        first
    );
}

#[tokio::test]
async fn short_names_can_be_improved_after_completion_without_new_plan_versions() {
    let f = Fixture::new("markdown").await;
    let task = f.task("A long implementation task").await;
    let claim = f.start(&task, "rename").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let old = f.service.store().snapshot().await.unwrap();
    f.clock.fetch_add(86400, Ordering::SeqCst);
    f.service
        .execute(
            json!({"command":"task.update","task":task,"name":"实现功能"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"name":"发布功能"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.tasks[0].name, "实现功能");
    assert_eq!(state.plans[0].version, old.plans[0].version);
    assert_eq!(
        state.plans[0].path,
        "Projects/demo/Tasks/260905-0001-实现功能.md"
    );
    assert_eq!(
        state.jobs[0].document_path,
        "Projects/demo/Jobs/260905-0001-发布功能.md"
    );
    let root = f.service.config().output_dir();
    assert!(!root.join(&old.plans[0].path).exists());
    assert!(
        std::fs::read_to_string(root.join(&state.plans[0].path))
            .unwrap()
            .contains("Implement and verify")
    );
    assert!(
        std::fs::read_to_string(root.join(&state.jobs[0].document_path))
            .unwrap()
            .contains("实现功能")
    );
}

#[tokio::test]
async fn plans_merge_authored_properties_and_reject_metadata_only_execution() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Authored properties").await;
    let claim = f.claim(&task, "properties").await;
    let plan = f.service.execute(json!({"command":"plan.create","task":task,"body":"---\ntitle: Authored title\ntags:\n  - custom/plan\nowner: Alice\n---\n\n# Acceptance\nVerify the result."}),owner(&claim)).await.unwrap().result;
    let path = plan["absolute_path"].as_str().unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    assert_eq!(
        text.lines().filter(|line| *line == "---").count(),
        2,
        "{text}"
    );
    let (properties, _) = text
        .strip_prefix("---\n")
        .unwrap()
        .split_once("\n---\n")
        .unwrap();
    assert!(properties.contains("custom/plan"));
    assert!(properties.contains("agent/task"));
    assert!(properties.contains("Alice"));
    std::fs::write(path, format!("---\n{properties}\n---\n\n")).unwrap();
    assert!(
        f.service
            .execute(json!({"command":"task.start","task":task}), owner(&claim))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pending_plan_publication_survives_failure_and_rejected_writers() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Recover publication").await;
    let claim = f.claim(&task, "publication").await;
    let plan = f.plan(&task).await;
    let path = std::path::Path::new(plan["absolute_path"].as_str().unwrap());
    let saved = path.with_extension("saved");
    std::fs::rename(path, &saved).unwrap();
    std::fs::create_dir(path).unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"plan.revise","task":task,"body":"# Latest committed body"}),
            owner(&claim),
        )
        .await
        .unwrap();
    assert!(result.projection_pending.is_some());
    assert!(
        f.service
            .execute(
                json!({"command":"plan.revise","task":task,"body":"# Unauthorized"}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    std::fs::remove_dir(path).unwrap();
    std::fs::rename(saved, path).unwrap();
    f.service.sync().await.unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.ends_with("# Latest committed body"));
    assert!(!text.contains("Unauthorized"));
    assert_eq!(f.service.store().snapshot().await.unwrap().plans.len(), 1);
}

#[tokio::test]
async fn a_renamed_plan_never_overwrites_an_unmanaged_note() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Original").await;
    let claim = f.claim(&task, "collision").await;
    let plan = f.plan(&task).await;
    let destination = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Tasks/260905-0001-Personal.md");
    std::fs::write(&destination, "# Personal note\nKeep this.").unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"task.update","task":task,"name":"Personal"}),
            owner(&claim),
        )
        .await
        .unwrap();
    assert!(result.projection_pending.is_some());
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "# Personal note\nKeep this."
    );
    assert!(std::path::Path::new(plan["absolute_path"].as_str().unwrap()).exists());
    std::fs::rename(&destination, destination.with_extension("saved")).unwrap();
    f.service.sync().await.unwrap();
    assert!(
        std::fs::read_to_string(destination)
            .unwrap()
            .contains("Implement and verify")
    );
}
