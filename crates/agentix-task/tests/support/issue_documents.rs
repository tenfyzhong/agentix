use super::*;

#[tokio::test]
async fn global_issue_base_exposes_cross_project_table_board_and_archive_filters() {
    let f = Fixture::new().await;
    let first = create_issue(&f).await;
    std::fs::create_dir_all(f.dir.path().join("second")).unwrap();
    let second_project = f.service.execute(json!({"command":"project.register","root":f.dir.path().join("second"),"name":"Second"}), WriteOptions::default()).await.unwrap().result;
    let second = f
        .service
        .execute(
            json!({"command":"issue.create","project":second_project["id"],"title":"Second issue"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let source = read(&f, "Issue.base");
    let base: Value = serde_yaml::from_str(&source).unwrap();
    assert!(
        f.service
            .config()
            .output_dir()
            .join("Dashboard.base")
            .exists()
    );
    assert_eq!(
        base["filters"]["and"][0]["or"],
        json!([
            "file.inFolder(\"Tasks ☃/Projects\")",
            "file.inFolder(\"Archived Projects\")"
        ])
    );
    assert!(base["filters"].to_string().contains("agent/issue"));
    assert!(!base["filters"].to_string().contains(&f.project));
    let views = base["views"].as_array().unwrap();
    assert!(
        views
            .iter()
            .any(|v| v["type"] == "table" && v["name"] == "Issues")
    );
    let board = views.iter().find(|v| v["type"] == "kanban").unwrap();
    assert_eq!(board["groupOrder"], json!(agentix_task::IssueStatus::ALL));
    for view in views {
        assert!(view.get("limit").is_none());
        assert!(
            view["order"]
                .as_array()
                .unwrap()
                .contains(&json!("project_id"))
        );
        assert!(view["order"].as_array().unwrap().contains(&json!("status")));
    }
    assert!(
        views
            .iter()
            .any(|v| v["filters"] == json!({"and":["archived == true"]}))
    );
    for (item, project) in [
        (&first, f.project.as_str()),
        (&second, second_project["id"].as_str().unwrap()),
    ] {
        let doc = read(&f, item["document_path"].as_str().unwrap());
        assert!(doc.contains(project));
        assert!(!doc.contains("projects:"), "Issues expose only project_id");
    }
}

#[tokio::test]
async fn global_issue_base_keeps_saved_filters_through_mutation_and_full_sync() {
    let f = Fixture::new().await;
    let item = create_issue(&f).await;
    let mut base: Value = serde_yaml::from_str(&read(&f, "Issue.base")).unwrap();
    base["views"][0]["filters"] =
        json!({"and":[format!("project_id == {}",json!(f.project)),"status == \"ACTIVE\""]});
    let saved = serde_yaml::to_string(&base).unwrap();
    let path = f.service.config().output_dir().join("Issue.base");
    std::fs::write(&path, &saved).unwrap();
    f.service
        .execute(
            json!({"command":"issue.update","issue":item["id"],"title":"Updated requirement"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), saved);
}

#[tokio::test]
async fn global_issue_base_incremental_refresh_preserves_registered_views_with_or_without_marker() {
    for marker in ["", "# taskix-generated: global-issues\n"] {
        let f = Fixture::new().await;
        let item = create_issue(&f).await;
        let path = f.service.config().output_dir().join("Issue.base");
        let mut base: Value = serde_yaml::from_str(&read(&f, "Issue.base")).unwrap();
        base["views"][0]["name"] = json!("My saved view");
        base["views"][0]["filters"] = json!({"and":["status == \"ACTIVE\""]});
        let saved = format!("{marker}{}", serde_yaml::to_string(&base).unwrap());
        std::fs::write(&path, &saved).unwrap();
        let outcome = f
            .service
            .execute(
                json!({"command":"issue.update","issue":item["id"],"title":"Updated requirement"}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.projection_pending, None,
            "registered Base must not block incremental publication"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            saved,
            "incremental publication must preserve saved views"
        );
        f.service.sync_pending_documents().await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        f.service.sync().await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
    }
}

#[tokio::test]
async fn global_issue_base_upgrade_creates_missing_view_and_preserves_user_collision() {
    let f = Fixture::new().await;
    let path = f.service.config().output_dir().join("Issue.base");
    let mut registry = f
        .service
        .store()
        .metadata("documents")
        .await
        .unwrap()
        .unwrap();
    registry
        .as_object_mut()
        .unwrap()
        .retain(|_, value| value != "Issue.base");
    f.service
        .store()
        .set_metadata("documents", &registry)
        .await
        .unwrap();
    std::fs::write(&path, "views: []\n").unwrap();
    let result = f.service.sync().await;
    assert!(
        result.is_err(),
        "an existing user Base must not be replaced"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "views: []\n");
    std::fs::remove_file(&path).unwrap();
    f.service.sync().await.unwrap();
    assert!(
        path.exists(),
        "existing installations receive the root Issue Base on sync"
    );
}

#[tokio::test]
async fn milestone_goal_authoring_survives_sync_and_metadata_updates() {
    let f = Fixture::new().await;
    let milestone = f.service.execute(json!({"command":"milestone.create","project":f.project,"title":"MVP","goal":"Original goal"}), WriteOptions::default()).await.unwrap().result;
    let path = f
        .service
        .config()
        .output_dir()
        .join(milestone["document_path"].as_str().unwrap());
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Original goal", "Authored goal");
    std::fs::write(&path, edited).unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(
        f.service
            .store()
            .milestone_record(milestone["id"].as_str().unwrap())
            .await
            .unwrap()
            .goal,
        "Authored goal"
    );
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains("Authored goal")
    );
}

#[tokio::test]
async fn issue_submission_at_authored_revision_imports_body_without_false_conflict() {
    let f = Fixture::new().await;
    let item = create_issue(&f).await;
    let path = f
        .service
        .config()
        .output_dir()
        .join(item["document_path"].as_str().unwrap());
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Allow users to sign in.", "Sign in with a passkey.");
    std::fs::write(&path, edited).unwrap();
    let outcome = f
        .service
        .execute(
            json!({"command":"issue.submit","issue":item["id"]}),
            WriteOptions {
                expected_revision: item["revision"].as_i64(),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome.result["status"], "PENDING_REVIEW");
    assert_eq!(outcome.result["requirement"], "Sign in with a passkey.");
}

#[tokio::test]
async fn milestone_rename_updates_linked_issue_notes_and_imports_its_goal_at_guarded_revision() {
    let f = Fixture::new().await;
    let milestone = f.service.execute(json!({"command":"milestone.create","project":f.project,"title":"MVP","goal":"Original"}), WriteOptions::default()).await.unwrap().result;
    let issue = create_issue(&f).await;
    f.service
        .execute(
            json!({"command":"issue.update","issue":issue["id"],"milestone":milestone["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let path = f
        .service
        .config()
        .output_dir()
        .join(milestone["document_path"].as_str().unwrap());
    std::fs::write(
        &path,
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("Original", "Authored"),
    )
    .unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"milestone.update","milestone":milestone["id"],"name":"Launch"}),
            WriteOptions {
                expected_revision: milestone["revision"].as_i64(),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.result["goal"], "Authored");
    let doc = read(&f, issue["document_path"].as_str().unwrap());
    assert!(doc.contains("Launch"));
    assert!(!doc.contains("MVP"));
}

async fn create_issue(f: &Fixture) -> Value {
    f.service.execute(json!({"command":"issue.create","project":f.project,"title":"Login",
        "requirement":"Allow users to sign in.","acceptance":"- [ ] Valid users succeed.","notes":"Keep browser sessions."}),WriteOptions::default()).await.unwrap().result
}

fn read(f: &Fixture, path: &str) -> String {
    std::fs::read_to_string(f.service.config().output_dir().join(path)).unwrap()
}

#[tokio::test]
async fn issue_document_uses_fixed_template_and_shared_issue_base_filters() {
    let f = Fixture::new().await;
    let item = create_issue(&f).await;
    let doc = read(&f, item["document_path"].as_str().unwrap());
    for section in [
        "## Requirement",
        "## Acceptance Criteria",
        "## Notes",
        "## Related Jobs",
    ] {
        assert!(doc.contains(section));
    }
    assert!(doc.contains("Allow users to sign in."));
    assert!(doc.contains("- [ ] Valid users succeed."));
    assert!(doc.contains("Issue.base#Related Jobs"));
    assert!(
        !f.service
            .config()
            .output_dir()
            .join("Projects/demo/Issue Jobs.base")
            .exists()
    );
    let base: Value = serde_yaml::from_str(&read(&f, "Projects/demo/Issue.base")).unwrap();
    assert_eq!(
        base["filters"]["and"],
        json!([format!("project_id == {}", json!(f.project))])
    );
    let jobs = base["views"]
        .as_array()
        .unwrap()
        .iter()
        .find(|view| view["name"] == "Related Jobs")
        .unwrap();
    let filters = jobs["filters"]["and"].as_array().unwrap();
    assert!(filters.contains(&json!(
        "file.hasTag(\"agent/job\") || file.hasTag(\"agent/archived/job\")"
    )));
    assert!(filters.contains(&json!("issue_id == this.id")));
    assert!(!filters.contains(&json!("archived != true")));
    assert_eq!(jobs["groupOrder"], json!(agentix_task::JobStatus::ALL));
    let board = read(&f, "Projects/demo/Board.md");
    assert!(board.contains("Issue.base#Issue board"));
}

#[tokio::test]
async fn issue_document_preserves_authoring_imports_before_submit_and_updates_metadata() {
    let f = Fixture::new().await;
    let item = create_issue(&f).await;
    let id = item["id"].as_str().unwrap();
    let path = f
        .service
        .config()
        .output_dir()
        .join(item["document_path"].as_str().unwrap());
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Allow users to sign in.", "Support passwordless sign-in.");
    std::fs::write(&path, edited).unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(
        f.service
            .store()
            .issue_record(id)
            .await
            .unwrap()
            .requirement,
        "Support passwordless sign-in."
    );
    let pending = f
        .service
        .execute(
            json!({"command":"issue.submit","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(pending["status"], "PENDING_REVIEW");
    let doc = std::fs::read_to_string(&path).unwrap();
    assert!(doc.contains("Support passwordless sign-in."));
    let yaml = doc
        .strip_prefix("---\n")
        .unwrap()
        .split_once("\n---\n")
        .unwrap()
        .0;
    assert_eq!(
        serde_yaml::from_str::<Value>(yaml).unwrap()["status"],
        "PENDING_REVIEW"
    );
    let note = f.service.obsidian_note(id).await.unwrap();
    assert_eq!(note["kind"], "issue");
    assert_eq!(note["status"], "PENDING_REVIEW");
}

#[tokio::test]
async fn issue_document_malformed_markers_reject_submission_without_overwrite() {
    let f = Fixture::new().await;
    let item = create_issue(&f).await;
    let path = f
        .service
        .config()
        .output_dir()
        .join(item["document_path"].as_str().unwrap());
    let malformed = std::fs::read_to_string(&path)
        .unwrap()
        .replace("<!-- taskix:requirement:end -->", "");
    std::fs::write(&path, &malformed).unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"issue.submit","issue":item["id"]}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), malformed);
    assert_eq!(
        f.service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .status
            .to_string(),
        "TODO"
    );
}

#[tokio::test]
async fn milestone_document_progress_counts_issue_acceptance_and_uses_shared_filters() {
    let f = Fixture::new().await;
    let milestone=f.service.execute(json!({"command":"milestone.create","project":f.project,"title":"MVP","goal":"Ship login"}),WriteOptions::default()).await.unwrap().result;
    let item = create_issue(&f).await;
    f.service
        .execute(
            json!({"command":"issue.update","issue":item["id"],"milestone":milestone["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let path = milestone["document_path"].as_str().unwrap();
    let doc = read(&f, path);
    assert!(doc.contains("Ship login"));
    assert!(doc.contains("Issue.base#Milestone Issues"));
    assert!(doc.contains("total_issues: 1"));
    assert!(doc.contains("completed_issues: 0"));
    f.service
        .execute(
            json!({"command":"issue.submit","issue":item["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"issue.approve","issue":item["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(read(&f, path).contains("completed_issues: 1"));
}

#[tokio::test]
async fn issue_document_recovers_committed_body_after_crash_before_publication() {
    let f = Fixture::new().await;
    let item = create_issue(&f).await;
    let id = item["id"].as_str().unwrap();
    f.service.store().execute(json!({"command":"issue.update","issue":id,"requirement":"Publish the committed new requirement."}),WriteOptions::default()).await.unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(
        f.service
            .store()
            .issue_record(id)
            .await
            .unwrap()
            .requirement,
        "Publish the committed new requirement."
    );
    assert!(
        read(&f, item["document_path"].as_str().unwrap())
            .contains("Publish the committed new requirement.")
    );
}

#[tokio::test]
async fn issue_empty_project_creates_folders_for_context_menu_creation() {
    let f = Fixture::new().await;
    let output = f.service.config().output_dir();
    assert!(output.join("Projects/demo/Issues").is_dir());
    assert!(output.join("Projects/demo/Milestones").is_dir());
}

#[tokio::test]
async fn job_issue_links_match_bridge_and_follow_renames_and_unlink() {
    let f = Fixture::new().await;
    let item = create_issue(&f).await;
    let before = f.service.obsidian_note(&f.job).await.unwrap();
    assert!(before["properties"].get("issue").is_some());
    assert!(before["properties"]["issue"].is_null());
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"issue":item["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    for name in ["Login", "Renamed requirement"] {
        f.service
            .execute(
                json!({"command":"issue.update","issue":item["id"],"name":name}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let note = f.service.obsidian_note(&f.job).await.unwrap();
        assert_eq!(note["properties"]["issue_id"], item["id"]);
        assert!(note["properties"]["issue"].as_str().unwrap().contains(name));
        let job = f.service.store().job_record(&f.job).await.unwrap();
        let source = read(&f, &job.document_path);
        let front = source.split("---").nth(1).unwrap();
        let properties: Value = serde_yaml::from_str(front).unwrap();
        assert_eq!(properties["issue"], note["properties"]["issue"]);
        let snapshot = f.service.obsidian_snapshot().await.unwrap();
        let exported = snapshot["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == f.job)
            .unwrap();
        assert_eq!(exported["properties"]["issue"], properties["issue"]);
    }
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"issue":null}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let note = f.service.obsidian_note(&f.job).await.unwrap();
    assert!(note["properties"]["issue"].is_null());
    assert!(note["properties"]["issue_id"].is_null());
}

#[tokio::test]
async fn entity_project_metadata_drops_legacy_links_and_preserves_project_id() {
    let f = Fixture::new().await;
    let issue = create_issue(&f).await;
    let milestone = f
        .service
        .execute(
            json!({"command":"milestone.create","project":f.project,"title":"MVP"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let task = f.task("Metadata").await;
    let snapshot = f.service.obsidian_snapshot().await.unwrap();
    let ids = [
        &f.job,
        issue["id"].as_str().unwrap(),
        milestone["id"].as_str().unwrap(),
        &task,
    ];
    let mut paths = Vec::new();
    for id in ids {
        let note = snapshot["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .unwrap();
        let path = f
            .service
            .config()
            .documents
            .root
            .join(note["path"].as_str().unwrap());
        let source = std::fs::read_to_string(&path).unwrap();
        let mut props: Value = serde_yaml::from_str(source.split("---").nth(1).unwrap()).unwrap();
        assert!(
            props.get("projects").is_none(),
            "new {id} has no projects field"
        );
        props["projects"] = json!(["[[Legacy Project]]"]);
        props["custom"] = json!("keep");
        let body = source.splitn(3, "---").nth(2).unwrap();
        std::fs::write(
            &path,
            format!("---\n{}---{}", serde_yaml::to_string(&props).unwrap(), body),
        )
        .unwrap();
        paths.push(path);
    }
    f.service.sync().await.unwrap();
    for path in paths {
        let source = std::fs::read_to_string(path).unwrap();
        let props: Value = serde_yaml::from_str(source.split("---").nth(1).unwrap()).unwrap();
        assert!(props.get("projects").is_none());
        assert_eq!(props["project_id"], f.project);
        assert_eq!(props["custom"], "keep");
    }
}

#[tokio::test]
async fn registered_bases_migrate_project_columns_without_losing_saved_filters() {
    let f = Fixture::new().await;
    let path = f.service.config().output_dir().join("Issue.base");
    let mut base: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    base["views"][0]["order"] = json!(["formula.name", "projects", "status"]);
    base["views"][0]["filters"] = json!({"and":["status == \"ACTIVE\"", "projects.contains(link(\"Tasks ☃/Projects/demo/Board\"))"]});
    base["views"][2]["groupBy"]["property"] = json!("note.projects");
    std::fs::write(&path, serde_yaml::to_string(&base).unwrap()).unwrap();
    f.service.sync().await.unwrap();
    let migrated: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        migrated["views"][0]["order"],
        json!(["formula.name", "project_id", "status"])
    );
    assert_eq!(
        migrated["views"][0]["filters"],
        json!({"and":["status == \"ACTIVE\"", format!("(project_id == {})", json!(f.project))]})
    );
    assert_eq!(
        migrated["views"][2]["groupBy"]["property"],
        "note.project_id"
    );
}

#[tokio::test]
async fn task_job_property_uses_filename_after_creation_sync_and_job_rename() {
    let f = Fixture::new().await;
    let task = f.task("Linked task").await;
    let note = f.service.obsidian_note(&task).await.unwrap();
    let path = f
        .service
        .config()
        .documents
        .root
        .join(note["path"].as_str().unwrap());
    for name in [None, Some("Issue 项目管理"), Some("Renamed delivery")] {
        if let Some(name) = name {
            f.service
                .execute(
                    json!({"command":"job.update","job":f.job,"name":name}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
        }
        let job = f.service.store().job_record(&f.job).await.unwrap();
        let stem = std::path::Path::new(&job.document_path)
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap();
        let expected = format!("[[{stem}]]");
        let source = std::fs::read_to_string(&path).unwrap();
        let mut props: Value = serde_yaml::from_str(source.split("---").nth(1).unwrap()).unwrap();
        assert_eq!(props["job"], expected);
        assert_eq!(props["job_id"], f.job);
        props["job"] = json!(format!(
            "[[Tasks ☃/{}]]",
            job.document_path.trim_end_matches(".md")
        ));
        let body = source.splitn(3, "---").nth(2).unwrap();
        std::fs::write(
            &path,
            format!("---\n{}---{}", serde_yaml::to_string(&props).unwrap(), body),
        )
        .unwrap();
        f.service.sync().await.unwrap();
        let source = std::fs::read_to_string(&path).unwrap();
        let props: Value = serde_yaml::from_str(source.split("---").nth(1).unwrap()).unwrap();
        assert_eq!(props["job"], expected);
        assert_eq!(props["job_id"], f.job);
    }
}

#[tokio::test]
async fn review_content_changes_require_reopening_and_invalidate_pending_acceptance() {
    let f = Fixture::new().await;
    for terminal in ["issue.approve", "issue.cancel"] {
        let item = create_issue(&f).await;
        let id = item["id"].as_str().unwrap();
        f.service
            .execute(
                json!({"command":"issue.submit","issue":id}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        f.service
            .execute(
                json!({"command":terminal,"issue":id}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let result = f
            .service
            .execute(
                json!({"command":"issue.update","issue":id,"requirement":"Unreviewed"}),
                WriteOptions::default(),
            )
            .await;
        assert!(result.is_err(), "terminal content requires reopening");
        f.service
            .execute(
                json!({"command":"issue.reopen","issue":id}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        f.service
            .execute(
                json!({"command":"issue.update","issue":id,"requirement":"Reopened"}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let item = create_issue(&f).await;
    let id = item["id"].as_str().unwrap();
    let pending = f
        .service
        .execute(
            json!({"command":"issue.submit","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let path = f
        .service
        .config()
        .output_dir()
        .join(item["document_path"].as_str().unwrap());
    std::fs::write(
        &path,
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("Allow users to sign in.", "Unreviewed requirement"),
    )
    .unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"issue.approve","issue":id}),
                WriteOptions {
                    expected_revision: pending["revision"].as_i64(),
                    ..WriteOptions::default()
                }
            )
            .await
            .is_err()
    );
    let updated = f.service.store().issue_record(id).await.unwrap();
    assert_eq!(updated.status, agentix_task::IssueStatus::Active);
    assert!(updated.pending_review_at.is_none());
}

#[tokio::test]
async fn delivery_updates_preserve_stale_revision_conflicts_for_authored_fields() {
    let f = Fixture::new().await;
    let issue = create_issue(&f).await;
    let milestone = f.service.execute(json!({"command":"milestone.create","project":f.project,"title":"Release","goal":"Original goal"}), WriteOptions::default()).await.unwrap().result;
    for (kind, item, field, original) in [
        ("issue", issue, "requirement", "Allow users to sign in."),
        ("milestone", milestone, "goal", "Original goal"),
    ] {
        let path = f
            .service
            .config()
            .output_dir()
            .join(item["document_path"].as_str().unwrap());
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)
                .unwrap()
                .replace(original, "Authored change"),
        )
        .unwrap();
        let mut request = json!({"command":format!("{kind}.update")});
        request[kind] = item["id"].clone();
        request[field] = json!("Stale overwrite");
        let result = f
            .service
            .execute(
                request,
                WriteOptions {
                    expected_revision: item["revision"].as_i64(),
                    ..WriteOptions::default()
                },
            )
            .await;
        assert!(
            result.is_err(),
            "{kind} must reject the stale content writer"
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("Authored change")
        );
    }
}

#[tokio::test]
async fn pending_delivery_publication_rejects_intervening_authored_edits() {
    let f = Fixture::new().await;
    let issue = create_issue(&f).await;
    let milestone = f.service.execute(json!({"command":"milestone.create","project":f.project,"title":"Release","goal":"Original goal"}), WriteOptions::default()).await.unwrap().result;
    for (kind, item, field, original) in [
        ("issue", issue, "notes", "Keep browser sessions."),
        ("milestone", milestone, "goal", "Original goal"),
    ] {
        let path = f
            .service
            .config()
            .output_dir()
            .join(item["document_path"].as_str().unwrap());
        let mut request = json!({"command":format!("{kind}.update")});
        request[kind] = item["id"].clone();
        request[field] = json!("Committed replacement");
        f.service
            .store()
            .execute(request, WriteOptions::default())
            .await
            .unwrap();
        let edited = std::fs::read_to_string(&path)
            .unwrap()
            .replace(original, "Intervening edit");
        std::fs::write(&path, &edited).unwrap();
        assert!(
            f.service.sync_pending_documents().await.is_err(),
            "{kind} pending publication must detect conflict"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        // Resolving to the committed body permits the durable retry.
        std::fs::write(
            &path,
            edited.replace("Intervening edit", "Committed replacement"),
        )
        .unwrap();
        f.service.sync_pending_documents().await.unwrap();
    }
}

#[tokio::test]
async fn rebuild_publication_imports_authored_issue_edits_before_rendering() {
    let f = Fixture::new().await;
    let issue = create_issue(&f).await;
    let path = f
        .service
        .config()
        .output_dir()
        .join(issue["document_path"].as_str().unwrap());
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Keep browser sessions.", "Keep my offline edit.");
    std::fs::write(&path, &edited).unwrap();
    let mut conn = super::incremental::connection(&f).await;
    sqlx::query(
        "INSERT OR REPLACE INTO pending_documents(key,generation) VALUES('rebuild','audit')",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    f.service.sync_pending_documents().await.unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Keep my offline edit.")
    );
    assert_eq!(
        f.service
            .store()
            .issue_record(issue["id"].as_str().unwrap())
            .await
            .unwrap()
            .notes,
        "Keep my offline edit."
    );
}

#[tokio::test]
async fn milestone_pages_bound_goals_and_preserve_progress_and_order() {
    let f = Fixture::new().await;
    let mut stages = Vec::new();
    for order in 0..8 {
        stages.push(f.service.store().execute(json!({"command":"milestone.create","project":f.project,"title":format!("Release {order}"),"order":order,"goal":"界".repeat(10_000)}), WriteOptions::default()).await.unwrap().result);
    }
    f.service.store().execute(json!({"command":"issue.create","project":f.project,"title":"Accepted","milestone":stages[6]["id"]}), WriteOptions::default()).await.unwrap();
    let page = f
        .service
        .store()
        .milestone_page(&f.project, 1, 6)
        .await
        .unwrap();
    assert_eq!(page["total"], 8);
    assert_eq!(page["page"], 1);
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["items"][0]["id"], stages[6]["id"]);
    assert_eq!(page["items"][0]["progress"]["total"], 1);
    assert!(page["items"][0]["goal"].as_str().unwrap().chars().count() <= 501);
    let last = f
        .service
        .store()
        .milestone_page(&f.project, usize::MAX, 6)
        .await
        .unwrap();
    assert_eq!(last["page"], 1);
    assert!(
        f.service
            .store()
            .milestone_page(&f.project, 0, 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn milestone_progress_matches_detail_pages_and_full_rebuild_with_archived_issues() {
    let f = Fixture::new().await;
    let mut stages = Vec::new();
    for title in ["Mixed", "Empty", "Other"] {
        stages.push(
            f.service
                .execute(
                    json!({"command":"milestone.create","project":f.project,"title":title}),
                    WriteOptions::default(),
                )
                .await
                .unwrap()
                .result,
        );
    }
    for (index, status) in ["TODO", "COMPLETED", "CANCELLED"].into_iter().enumerate() {
        let issue = f.service.execute(json!({"command":"issue.create","project":f.project,"title":format!("Item {index}"),"milestone":stages[0]["id"],"requirement":"Deliver a feature","acceptance":"Verify the feature"}), WriteOptions::default()).await.unwrap().result;
        if status == "COMPLETED" {
            for command in ["issue.submit", "issue.approve", "issue.archive"] {
                f.service
                    .execute(
                        json!({"command":command,"issue":issue["id"]}),
                        WriteOptions::default(),
                    )
                    .await
                    .unwrap();
            }
        } else if status == "CANCELLED" {
            f.service
                .execute(
                    json!({"command":"issue.cancel","issue":issue["id"]}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
        }
    }
    f.service.execute(json!({"command":"issue.create","project":f.project,"title":"Elsewhere","milestone":stages[2]["id"]}), WriteOptions::default()).await.unwrap();
    f.service
        .execute(
            json!({"command":"issue.create","project":f.project,"title":"Unassigned"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let page = f
        .service
        .store()
        .milestone_page(&f.project, 0, 6)
        .await
        .unwrap();
    for (stage, expected) in stages.iter().zip([(3, 1, 1), (0, 0, 0), (1, 0, 0)]) {
        let detail = f
            .service
            .store()
            .milestone_detail(stage["id"].as_str().unwrap())
            .await
            .unwrap();
        let summary = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == stage["id"])
            .unwrap();
        for (field, count) in [
            ("total", expected.0),
            ("completed", expected.1),
            ("cancelled", expected.2),
        ] {
            assert_eq!(detail["progress"][field], count);
            assert_eq!(summary["progress"][field], count);
        }
        assert_eq!(
            detail["progress"]["remaining"],
            expected.0 - expected.1 - expected.2
        );
        let path = stage["document_path"].as_str().unwrap();
        let before = read(&f, path);
        f.service.sync().await.unwrap();
        let after = read(&f, path);
        for (field, count) in [
            ("total_issues", expected.0),
            ("completed_issues", expected.1),
            ("cancelled_issues", expected.2),
        ] {
            let property = format!("{field}: {count}");
            assert!(before.contains(&property));
            assert!(after.contains(&property));
        }
    }
}
