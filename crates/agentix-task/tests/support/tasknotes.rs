use super::*;
use sqlx::Connection;

fn properties(document: &str) -> Value {
    serde_yaml::from_str(
        document
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0,
    )
    .unwrap()
}

fn base(document: &str) -> Value {
    serde_yaml::from_str(
        document
            .split_once("```base\n")
            .expect("embedded TaskNotes Base")
            .1
            .split_once("\n```")
            .unwrap()
            .0,
    )
    .unwrap()
}

#[tokio::test]
async fn board_contains_project_metadata_and_is_the_only_project_link_target() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
        let task = f.task("Linked task").await;
        let root = f.service.config().output_dir();
        let board_path = root.join("Projects/demo/Board.md");
        let board = std::fs::read_to_string(&board_path).unwrap();
        let props = properties(&board);
        let state = f.service.store().snapshot().await.unwrap();
        let project = &state.projects[0];
        assert_eq!(props["id"], project.id);
        assert_eq!(props["name"], project.name);
        assert_eq!(props["root"], json!(project.root));
        assert_eq!(props["remote"], json!(project.remote));
        assert_eq!(props["revision"], project.revision);
        assert_eq!(props["status"], "ACTIVE");
        assert!(props["archived_at"].is_null());
        assert_eq!(props["sync_status"], "synced");
        assert_eq!(
            props["sync_sequence"],
            f.service.store().latest_sequence().await.unwrap()
        );
        assert_eq!(props["tags"], json!(["agent/project", "agent/board"]));
        assert!(!root.join("Projects/demo/meta.md").exists());
        assert!(!board.contains("|Project]]") && !board.contains("[Project]("));
        assert_eq!(base(&board)["views"][0]["type"], "tasknotesKanban");
        let dashboard = std::fs::read_to_string(root.join(f.dashboard_file())).unwrap();
        assert!(!dashboard.contains("/meta"));
        if format == "markdown" {
            assert_eq!(dashboard.matches("Projects/demo/Board").count(), 1);
        } else {
            assert!(dashboard.contains("link(file.path, note.name)"));
        }
        let task_path = root.join("Projects/demo/Tasks/260905-0001-Linked task.md");
        assert_eq!(
            properties(&std::fs::read_to_string(task_path).unwrap())["projects"],
            json!(["[[Tasks ☃/Projects/demo/Board]]"])
        );
        f.service
            .execute(
                json!({"command":"task.cancel","task":task,"reason":"Closed"}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        f.service
            .execute(
                json!({"command":"job.cancel","job":f.job}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        f.service
            .execute(
                json!({"command":"project.archive","project":f.project}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let archived = properties(&std::fs::read_to_string(&board_path).unwrap());
        assert_eq!(archived["status"], "ARCHIVED");
        assert!(archived["archived_at"].is_string());
        assert!(
            !std::fs::read_to_string(root.join(f.dashboard_file()))
                .unwrap()
                .contains("Projects/demo/Board")
        );
        f.service
            .execute(
                json!({"command":"project.unarchive","project":f.project}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            properties(&std::fs::read_to_string(board_path).unwrap())["status"],
            "ACTIVE"
        );
    }
}

#[tokio::test]
async fn legacy_project_meta_is_deleted_only_after_board_publication_and_retry_is_safe() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
        let root = f.service.config().output_dir();
        let relative = "Projects/demo/meta.md";
        let meta = root.join(relative);
        std::fs::write(
            &meta,
            "---\ntaskcli-generated: true\n---\n\n# Legacy project metadata\n",
        )
        .unwrap();
        let mut documents = f
            .service
            .store()
            .metadata("documents")
            .await
            .unwrap()
            .unwrap();
        documents[format!("meta:{}", f.project)] = json!(relative);
        f.service
            .store()
            .set_metadata("documents", &documents)
            .await
            .unwrap();
        let board = root.join("Projects/demo/Board.md");
        std::fs::remove_file(&board).unwrap();
        std::fs::create_dir(&board).unwrap();
        assert!(f.service.sync().await.is_err());
        assert!(
            meta.exists(),
            "retain legacy metadata when Board cannot be written"
        );
        std::fs::remove_dir(&board).unwrap();
        f.service.sync().await.unwrap();
        assert!(!meta.exists());
        assert_eq!(
            properties(&std::fs::read_to_string(&board).unwrap())["id"],
            f.project
        );
        assert!(
            f.service
                .store()
                .metadata("documents")
                .await
                .unwrap()
                .unwrap()
                .get(format!("meta:{}", f.project))
                .is_none()
        );
        // Recover if the old file was removed before path bookkeeping was saved.
        f.service
            .store()
            .set_metadata("documents", &documents)
            .await
            .unwrap();
        f.service.sync().await.unwrap();
        assert!(!meta.exists());
        assert_eq!(
            properties(&std::fs::read_to_string(board).unwrap())["id"],
            f.project
        );
    }
}

#[tokio::test]
async fn task_dependencies_are_projected_before_planning_and_follow_cli_changes() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
        let first = f.task("First prerequisite").await;
        let second = f.task("Second prerequisite").await;
        let task = f.task("Dependent").await;
        let path = f
            .service
            .config()
            .output_dir()
            .join("Projects/demo/Tasks/260905-0003-Dependent.md");
        let read = || properties(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(read()["dependencies"], json!([]));
        for dependency in [&first, &second] {
            f.service
                .execute(
                    json!({"command":"task.depend","task":task,"dependency":dependency}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
        }
        assert_eq!(read()["dependencies"], json!([first, second]));
        assert!(f.service.store().snapshot().await.unwrap().plans.is_empty());
        assert!(read()["plan_id"].is_null());
        for dependency in [&first, &second] {
            f.service
                .execute(
                    json!({"command":"task.undepend","task":task,"dependency":dependency}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
            let expected = if dependency == &first {
                json!([second])
            } else {
                json!([])
            };
            assert_eq!(read()["dependencies"], expected);
        }
    }
}

#[tokio::test]
async fn task_dependency_properties_are_managed_and_cannot_bypass_start() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
        let dependency = f.task("Prerequisite").await;
        let task = f.task("Dependent").await;
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
                json!({"command":"plan.create","task":task,"body":"---\ndependencies: []\n---\n\nKeep this plan.\n"}),
                owner(&claim),
            )
            .await
            .unwrap();
        let note = f.service.plan(&task).await.unwrap();
        assert_eq!(note["properties"]["dependencies"], json!([dependency]));
        let path = note["absolute_path"].as_str().unwrap();
        std::fs::write(path, "---\ndependencies: []\n---\n\nKeep this plan.\n").unwrap();
        assert!(
            f.service
                .execute(json!({"command":"task.start","task":task}), owner(&claim))
                .await
                .unwrap_err()
                .to_string()
                .contains("dependencies")
        );
        f.service.sync().await.unwrap();
        let note = f.service.plan(&task).await.unwrap();
        assert_eq!(note["properties"]["dependencies"], json!([dependency]));
        assert_eq!(note["body"], "Keep this plan.\n");
        let upstream = f.start(&dependency, "upstream").await;
        f.service
            .execute(
                json!({"command":"task.done","task":dependency}),
                owner(&upstream),
            )
            .await
            .unwrap();
        f.service
            .execute(json!({"command":"task.start","task":task}), owner(&claim))
            .await
            .unwrap();
        assert_eq!(
            f.service.plan(&task).await.unwrap()["properties"]["dependencies"],
            json!([dependency])
        );
    }
}

#[tokio::test]
async fn tasks_exist_before_planning_and_jobs_reference_notes_directly() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
        let id = f.task("Write tests").await;
        let output = f.service.config().output_dir();
        let path = output.join("Projects/demo/Tasks/260905-0001-Write tests.md");
        let doc = std::fs::read_to_string(&path).expect("every Task has a note before planning");
        let props = properties(&doc);
        assert_eq!(props["id"], id);
        assert_eq!(props["status"], "TODO");
        assert!(props.get("version").is_none());
        assert_eq!(props["tags"], json!(["agent/task", "task"]));
        assert_eq!(props["archived"], false);
        assert!(
            time::OffsetDateTime::parse(
                props["dateCreated"].as_str().unwrap(),
                &time::format_description::well_known::Rfc3339
            )
            .is_ok()
        );
        assert_eq!(props["dateCreated"], props["created_at"]);
        let state = f.service.store().snapshot().await.unwrap();
        let job = std::fs::read_to_string(output.join(&state.jobs[0].document_path)).unwrap();
        assert!(job.contains("260905-0001-Write tests"));
        assert!(!job.contains("- [ ]") && !job.contains("- [/]"));
        assert!(!job.contains("#agent/task"));
        let claim = f.claim(&id, "writer").await;
        let plan = f.plan(&id).await;
        assert_eq!(plan["absolute_path"], path.to_str().unwrap());
        assert!(!output.join("Projects/demo/Plans").exists());
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("Implement and verify."));
        assert_eq!(properties(&first)["id"], id);
        assert_eq!(properties(&first)["plan_id"], plan["id"]);
        assert!(properties(&first).get("version").is_none());
        f.service.execute(json!({"command":"plan.revise","task":id,"body":"# Revised\n\n## Goal\nShip.\n\n## Approach\nImplement.\n\n## Expected outcome\nWorks.\n\n## Validation\nRun tests.\n"}),owner(&claim)).await.unwrap();
        assert_eq!(
            properties(&std::fs::read_to_string(&path).unwrap())["revision"],
            f.service.store().snapshot().await.unwrap().tasks[0].revision
        );
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}

#[tokio::test]
async fn tasknotes_views_use_scoped_frontmatter_and_preserve_every_status() {
    let f = Fixture::new("obsidian").await;
    populate_board_states(&f).await;
    let root = f.service.config().output_dir().join("Projects/demo");
    let doc = std::fs::read_to_string(root.join("Board.md")).unwrap();
    let config = base(&doc);
    assert_eq!(config["views"][0]["type"], "tasknotesKanban");
    assert_eq!(config["views"][0]["groupBy"]["property"], "status");
    assert_eq!(
        config["filters"]["and"],
        json!([
            "file.folder == \"Tasks ☃/Projects/demo/Tasks\"",
            "file.hasTag(\"agent/task\")",
            format!("project_id == {:?}", f.project),
            "archived != true"
        ])
    );
    assert!(!doc.contains("kanban_plugin"));
    assert!(!doc.contains("```tasks"));
    assert!(!doc.contains("- ["));
    let board = base(&std::fs::read_to_string(root.join("Board.md")).unwrap());
    assert_eq!(
        board["views"][0]["columnOrder"]["status"],
        json!(task_status_names())
    );
    assert_eq!(board["views"][0]["hideEmptyColumns"], false);
    let state = f.service.store().snapshot().await.unwrap();
    for task in &state.tasks {
        let filename = format!("260905-{:04}-{}.md", task.sequence, task.name);
        let props =
            properties(&std::fs::read_to_string(root.join("Tasks").join(filename)).unwrap());
        assert_eq!(props["status"], task.status.to_string());
        assert_eq!(props["phase"], json!(task.phase));
        assert_eq!(props["archived"], false);
        if task.status == agentix_task::TaskStatus::Done {
            assert!(!props["completedDate"].is_null());
        }
    }
}

#[tokio::test]
async fn schema_six_plans_migrate_without_losing_authored_content() {
    let f = Fixture::new("obsidian").await;
    let id = f.task("Keep plan").await;
    f.claim(&id, "migrate").await;
    let plan = f.plan(&id).await;
    let root = f.service.config().output_dir();
    let old = "Projects/demo/Plans/260905-0001-Keep plan.md";
    std::fs::create_dir_all(root.join("Projects/demo/Plans")).unwrap();
    std::fs::write(root.join(old), "---\ntitle: Kept title\ntags: [agent/plan, research]\ncustom: preserved\n---\n\n# Original plan\n\n- [ ] Authored test checklist\n").unwrap();
    if plan["path"] != old {
        std::fs::remove_file(plan["absolute_path"].as_str().unwrap()).unwrap();
    }
    let mut pool = sqlx::SqliteConnection::connect(&format!(
        "sqlite:{}",
        f.service.config().storage.path.display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE plans SET data=json_set(data,'$.path',?) WHERE id=?")
        .bind(old)
        .bind(plan["id"].as_str().unwrap())
        .execute(&mut pool)
        .await
        .unwrap();
    let documents = json!({format!("plan:{}",plan["id"].as_str().unwrap()):old});
    // Preserve other managed paths and reproduce the version-six Plan entry.
    let previous: String =
        sqlx::query_scalar("SELECT value FROM projection_state WHERE key='documents'")
            .fetch_one(&mut pool)
            .await
            .unwrap();
    let mut previous: Value = serde_json::from_str(&previous).unwrap();
    previous
        .as_object_mut()
        .unwrap()
        .remove(&format!("task:{id}"));
    previous
        .as_object_mut()
        .unwrap()
        .extend(documents.as_object().unwrap().clone());
    sqlx::query("UPDATE projection_state SET value=? WHERE key='documents'")
        .bind(previous.to_string())
        .execute(&mut pool)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version=6")
        .execute(&mut pool)
        .await
        .unwrap();
    let service = Service::open(f.service.config().clone()).await.unwrap();
    service.sync().await.unwrap();
    let p = service.plan(&id).await.unwrap();
    assert_eq!(p["path"], "Projects/demo/Tasks/260905-0001-Keep plan.md");
    assert_eq!(p["properties"]["id"], id);
    assert_eq!(p["properties"]["title"], "Kept title");
    assert_eq!(p["properties"]["custom"], "preserved");
    assert_eq!(
        p["properties"]["tags"],
        json!(["research", "agent/task", "task"])
    );
    assert_eq!(
        p["body"],
        "# Original plan\n\n- [ ] Authored test checklist\n"
    );
    assert!(!root.join(old).exists());
    service.sync().await.unwrap();
    assert_eq!(service.plan(&id).await.unwrap()["body"], p["body"]);
}

#[tokio::test]
async fn task_tags_are_restored_on_sync_and_preserve_authored_tags() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
        let id = f.task("Tagged task").await;
        let claim = f.claim(&id, "writer").await;
        f.service
            .execute(
                json!({"command":"plan.create","task":id,"body":"---\ntags: [research, agent/task]\n---\n\nKeep this plan.\n"}),
                owner(&claim),
            )
            .await
            .unwrap();
        let note = f.service.plan(&id).await.unwrap();
        let path = note["absolute_path"].as_str().unwrap();
        // Simulate older notes and user edits, including a scalar tag.
        for tags in [
            "[research, agent/task]",
            "task",
            "[research, task, agent/plan]",
        ] {
            std::fs::write(path, format!("---\ntags: {tags}\n---\n\nKeep this plan.\n")).unwrap();
            for _ in 0..2 {
                f.service.sync().await.unwrap();
                let document = std::fs::read_to_string(path).unwrap();
                let props = properties(&document);
                let actual = props["tags"].as_array().unwrap();
                for tag in ["task", "agent/task"] {
                    assert_eq!(actual.iter().filter(|value| **value == tag).count(), 1);
                }
                assert_eq!(
                    actual.contains(&json!("research")),
                    tags.contains("research")
                );
                assert!(!actual.contains(&json!("agent/plan")));
                assert!(!actual.contains(&json!("archived")));
                assert!(document.contains("Keep this plan."));
            }
        }
    }
}

#[tokio::test]
async fn archive_and_delete_include_unplanned_task_notes() {
    let f = Fixture::new("markdown").await;
    let id = f.task("Unplanned").await;
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Tasks/260905-0001-Unplanned.md");
    assert!(path.exists());
    f.service
        .execute(
            json!({"command":"task.cancel","task":id,"reason":"Closed"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"job.archive","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        properties(&std::fs::read_to_string(&path).unwrap())["archived"],
        true
    );
    assert!(
        properties(&std::fs::read_to_string(&path).unwrap())["tags"]
            .as_array()
            .unwrap()
            .contains(&json!("archived"))
    );
    let archived = properties(&std::fs::read_to_string(&path).unwrap());
    for tag in ["task", "agent/task"] {
        assert!(archived["tags"].as_array().unwrap().contains(&json!(tag)));
    }
    f.service
        .execute(
            json!({"command":"job.unarchive","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        properties(&std::fs::read_to_string(&path).unwrap())["archived"],
        false
    );
    f.service
        .execute(
            json!({"command":"job.delete","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn renaming_an_unplanned_task_moves_its_note_and_updates_its_display_title() {
    let f = Fixture::new("obsidian").await;
    let id = f.task("Original task").await;
    let root = f.service.config().output_dir().join("Projects/demo/Tasks");
    let old = root.join("260905-0001-Original task.md");
    let content = std::fs::read_to_string(&old).unwrap();
    std::fs::write(&old, format!("{content}\nResearch to retain.\n")).unwrap();
    f.service
        .execute(
            json!({"command":"task.update","task":id,"name":"Renamed task"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let new = root.join("260905-0001-Renamed task.md");
    let doc = std::fs::read_to_string(new).unwrap();
    assert!(!old.exists());
    assert!(doc.contains("Research to retain."));
    assert_eq!(properties(&doc)["title"], "Renamed task");
}

#[tokio::test]
async fn sync_does_not_rewrite_unchanged_live_base_documents() {
    let f = Fixture::new("obsidian").await;
    f.task("Stable board").await;
    let board = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Board.md");
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    std::fs::File::options()
        .write(true)
        .open(&board)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(
        std::fs::metadata(&board).unwrap().modified().unwrap(),
        modified,
        "unchanged Base content must not trigger an Obsidian view reload"
    );
}

#[tokio::test]
async fn task_properties_use_local_time_and_only_task_revision() {
    let f = Fixture::new("obsidian").await;
    let id = f.task("Local times").await;
    let claim = f.start(&id, "local-time").await;
    f.service
        .execute(json!({"command":"task.done","task":id}), owner(&claim))
        .await
        .unwrap();
    let plan = f.service.plan(&id).await.unwrap();
    let path = plan["absolute_path"].as_str().unwrap();
    let legacy = std::fs::read_to_string(path)
        .unwrap()
        .replacen("---\n", "---\nversion: 999\n", 1);
    // Reproduce old version metadata without duplicate YAML keys.
    let legacy = legacy
        .lines()
        .filter(|line| !line.starts_with("version:") || *line == "version: 999")
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, legacy).unwrap();
    f.service.sync().await.unwrap();
    let props = properties(&std::fs::read_to_string(path).unwrap());
    assert!(props.get("version").is_none());
    assert_eq!(
        props["revision"],
        f.service.store().snapshot().await.unwrap().tasks[0].revision
    );
    let instant =
        time::OffsetDateTime::from_unix_timestamp(f.clock.load(Ordering::SeqCst)).unwrap();
    let expected = instant
        .to_offset(time::UtcOffset::local_offset_at(instant).unwrap())
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    for field in [
        "created_at",
        "updated_at",
        "started_at",
        "completed_at",
        "dateCreated",
        "dateModified",
        "completedDate",
    ] {
        assert_eq!(props[field], expected, "local timestamp in {field}");
    }
}

#[tokio::test]
async fn task_bodies_have_no_mandatory_sections_and_preserve_freeform_writing() {
    let f = Fixture::new("markdown").await;
    let id = f.task("Freeform").await;
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Tasks/260905-0001-Freeform.md");
    let initial = std::fs::read_to_string(path).unwrap();
    assert!(
        !initial.contains("## "),
        "do not prefill a fixed section template"
    );
    let claim = f.claim(&id, "freeform").await;
    let body = "An investigation log.\n\n> A useful observation.\n\nNext, verify the hypothesis.\n";
    f.service
        .execute(
            json!({"command":"plan.create","task":id,"body":body}),
            owner(&claim),
        )
        .await
        .unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(f.service.plan(&id).await.unwrap()["body"], body);
}

#[tokio::test]
async fn sync_removes_managed_task_lists_and_their_navigation_links() {
    let f = Fixture::new("obsidian").await;
    f.task("Keep note").await;
    let root = f.service.config().output_dir();
    let relative = "Projects/demo/Tasks.md";
    assert!(
        !root.join(relative).exists(),
        "new projects only provide Board"
    );
    std::fs::write(root.join(relative), "# Old task list\n").unwrap();
    let mut db = sqlx::SqliteConnection::connect(&format!(
        "sqlite:{}",
        f.service.config().storage.path.display()
    ))
    .await
    .unwrap();
    let raw: String =
        sqlx::query_scalar("SELECT value FROM projection_state WHERE key='documents'")
            .fetch_one(&mut db)
            .await
            .unwrap();
    let mut documents: Value = serde_json::from_str(&raw).unwrap();
    documents[format!("tasks:{}", f.project)] = json!(relative);
    sqlx::query("UPDATE projection_state SET value=? WHERE key='documents'")
        .bind(documents.to_string())
        .execute(&mut db)
        .await
        .unwrap();
    f.service.sync().await.unwrap();
    assert!(!root.join(relative).exists());
    assert_eq!(
        std::fs::read_dir(root.join("Projects/demo/Tasks"))
            .unwrap()
            .count(),
        1
    );
    for path in [f.dashboard_file(), "Projects/demo/Board.md"] {
        let doc = std::fs::read_to_string(root.join(path)).unwrap();
        assert!(!doc.contains("/Tasks|"));
        assert!(!doc.contains("Task list"));
        assert!(!doc.contains("Tasks.md"));
    }
}
