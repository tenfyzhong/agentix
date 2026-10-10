use super::*;

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
