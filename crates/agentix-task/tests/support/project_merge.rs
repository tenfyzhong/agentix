use super::*;

#[tokio::test]
#[allow(clippy::too_many_lines)] // Verify one merge preserves the complete related work graph and authored files.
async fn project_merge_preserves_work_and_removes_duplicate_folder() {
    let f = Fixture::new().await;
    let root = tempfile::tempdir().unwrap();
    let remote = "git@example.com:team/demo.git";
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .arg("init")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(["remote", "add", "origin", remote])
            .output()
            .unwrap()
            .status
            .success()
    );
    let duplicate = f
        .service
        .execute(
            json!({"command":"project.register","name":"demo","root":root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    // Model legacy duplicate records created before remote-based registration.
    let mut conn = super::incremental::connection(&f).await;
    sqlx::query("UPDATE projects SET data=json_set(data,'$.remote',?) WHERE id IN (?,?)")
        .bind(remote)
        .bind(&f.project)
        .bind(duplicate["id"].as_str().unwrap())
        .execute(&mut conn)
        .await
        .unwrap();
    let task = f.task("Keep plan").await;
    f.claim(&task, "merge-test").await;
    f.plan(&task).await;
    let other_job = f.service.execute(json!({"command":"job.create","project":duplicate["id"],"title":"Feature","goal":"Other authored goal"}), WriteOptions::default()).await.unwrap().result;
    let milestone=f.service.execute(json!({"command":"milestone.create","project":duplicate["id"],"title":"Milestone","goal":"Keep milestone goal"}),WriteOptions::default()).await.unwrap().result;
    let issue=f.service.execute(json!({"command":"issue.create","project":duplicate["id"],"title":"Issue","requirement":"Keep authored requirement","milestone":milestone["id"]}),WriteOptions::default()).await.unwrap().result;
    f.service.execute(json!({"command":"session.record","project":duplicate["id"],"session":"merge-memory","turn_id":"decision","messages":[{"id":"user","role":"user","text":"Preserve evidence"}]}), WriteOptions { session_ref: Some("merge-memory".into()), ..WriteOptions::default() }).await.unwrap();
    let sources_before = f.service.store().memory_sources(0, 100).await.unwrap();
    assert_eq!(sources_before.len(), 1);
    let folder = f.service.config().output_dir().join("Projects/demo-2");
    std::fs::write(folder.join("custom.md"), "Keep this custom note").unwrap();
    std::fs::create_dir(folder.join("custom")).unwrap();
    std::fs::write(folder.join("custom/Board.md"), "A personal board").unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"project.merge","project":f.project,"source":duplicate["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        result.projection_pending.is_none(),
        "{:?}",
        result.projection_pending
    );
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.projects.len(), 1);
    assert_eq!(
        f.service.store().project_merges().await.unwrap(),
        vec![(
            duplicate["id"].as_str().unwrap().to_owned(),
            f.project.clone()
        )]
    );
    let sources_after = f.service.store().memory_sources(0, 100).await.unwrap();
    assert_eq!(sources_after.len(), sources_before.len());
    for (before, after) in sources_before.iter().zip(&sources_after) {
        assert_eq!(before.receipt_id, after.receipt_id);
        assert_eq!(before.messages, after.messages);
        assert_eq!(after.project_id, f.project);
    }
    f.service
        .store()
        .verify_memory_sources(&sources_after)
        .await
        .unwrap();
    f.service.execute(json!({"command":"session.record","project":f.project,"session":"merge-memory","turn_id":"decision","messages":[{"id":"user","role":"user","text":"Preserve evidence"}]}), WriteOptions { session_ref: Some("merge-memory".into()), ..WriteOptions::default() }).await.unwrap();
    assert_eq!(
        f.service
            .store()
            .memory_sources(0, 100)
            .await
            .unwrap()
            .len(),
        sources_after.len()
    );
    assert_eq!(state.projects[0].key, "demo");
    assert_eq!(
        state.projects[0].root,
        root.path().canonicalize().unwrap().to_string_lossy()
    );
    assert!(state.jobs.iter().all(|j| j.project_id == f.project));
    assert!(state.jobs.iter().any(|j| j.id == other_job["id"]));
    assert_eq!(state.issues[0].project_id, f.project);
    assert_eq!(state.issues[0].id, issue["id"]);
    assert_eq!(state.milestones[0].project_id, f.project);
    let issue_text = std::fs::read_to_string(
        f.service
            .config()
            .output_dir()
            .join(&state.issues[0].document_path),
    )
    .unwrap();
    assert!(issue_text.contains("Keep authored requirement"));
    assert_eq!(state.leases.len(), 1);
    assert_eq!(state.plans.len(), 1);
    assert!(!folder.exists());
    assert_eq!(
        std::fs::read_to_string(
            f.service
                .config()
                .output_dir()
                .join("Projects/demo/custom/Board.md")
        )
        .unwrap(),
        "A personal board"
    );
    assert_eq!(
        std::fs::read_to_string(
            f.service
                .config()
                .output_dir()
                .join("Projects/demo/custom.md")
        )
        .unwrap(),
        "Keep this custom note"
    );
}

#[tokio::test]
async fn project_merge_rejects_different_remotes_without_mutating_state() {
    let f = Fixture::new().await;
    let root = tempfile::tempdir().unwrap();
    let other = f.service.execute(json!({"command":"project.register","name":"other","root":root.path(),"remote":"other"}), WriteOptions::default()).await.unwrap().result;
    let before = f.service.store().snapshot().await.unwrap();
    let error = f
        .service
        .execute(
            json!({"command":"project.merge","project":f.project,"source":other["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("same"), "{error}");
    let after = f.service.store().snapshot().await.unwrap();
    assert_eq!(before.projects, after.projects);
    assert_eq!(before.jobs, after.jobs);
}

#[tokio::test]
async fn project_merge_rejects_conflicting_authored_files_before_commit() {
    let f = Fixture::new().await;
    let root = tempfile::tempdir().unwrap();
    for args in [vec!["init"], vec!["remote", "add", "origin", "same"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let duplicate = f
        .service
        .execute(
            json!({"command":"project.register","name":"demo","root":root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let mut conn = super::incremental::connection(&f).await;
    sqlx::query("UPDATE projects SET data=json_set(data,'$.remote','same')")
        .execute(&mut conn)
        .await
        .unwrap();
    let output = f.service.config().output_dir();
    std::fs::write(output.join("Projects/demo/custom.md"), "first").unwrap();
    std::fs::write(output.join("Projects/demo-2/custom.md"), "second").unwrap();
    let before = f.service.store().snapshot().await.unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"project.merge","project":f.project,"source":duplicate["id"]}),
            WriteOptions::default(),
        )
        .await;
    assert!(
        result.is_err(),
        "conflict must reject before commit: {result:?}"
    );
    assert_eq!(
        before.projects,
        f.service.store().snapshot().await.unwrap().projects
    );
    assert_eq!(
        std::fs::read_to_string(output.join("Projects/demo-2/custom.md")).unwrap(),
        "second"
    );
}

#[tokio::test]
async fn project_merge_replay_keeps_original_identity_and_one_result() {
    let f = Fixture::new().await;
    let root = tempfile::tempdir().unwrap();
    for args in [vec!["init"], vec!["remote", "add", "origin", "same"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let duplicate = f
        .service
        .execute(
            json!({"command":"project.register","name":"demo","root":root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let mut conn = super::incremental::connection(&f).await;
    sqlx::query("UPDATE projects SET data=json_set(data,'$.remote','same')")
        .execute(&mut conn)
        .await
        .unwrap();
    let request = json!({"command":"project.merge","project":duplicate["id"],"source":f.project});
    let options = WriteOptions {
        idempotency_key: Some("merge-once".into()),
        ..WriteOptions::default()
    };
    let merged = f
        .service
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    assert!(merged.projection_pending.is_none());
    assert_eq!(merged.result["id"], f.project);
    let replay = f.service.execute(request, options).await.unwrap();
    assert_eq!(replay.result, merged.result);
    assert_eq!(replay.sequence, merged.sequence);
}

#[tokio::test]
async fn project_merge_cleans_both_suffixed_folders_and_recovers_after_restart() {
    let f = Fixture::new().await;
    let output = f.service.config().output_dir();
    std::fs::rename(output.join("Projects/demo"), output.join("Projects/demo-2")).unwrap();
    f.service.sync().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    for args in [vec!["init"], vec!["remote", "add", "origin", "same"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let duplicate = f
        .service
        .execute(
            json!({"command":"project.register","name":"demo-3","root":root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let mut conn = super::incremental::connection(&f).await;
    sqlx::query("UPDATE projects SET data=json_set(data,'$.remote','same')")
        .execute(&mut conn)
        .await
        .unwrap();
    std::fs::write(output.join("Projects/demo-2/extra.md"), "Retained extra").unwrap();
    let result = f
        .service
        .store()
        .execute(
            json!({"command":"project.merge","project":f.project,"source":duplicate["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(result.result["key"], "demo");
    // Reopen the database before publishing the durable merge journal.
    let service = Service::new(
        f.service.config().clone(),
        Store::open(&f.service.config().storage.path).await.unwrap(),
    )
    .unwrap();
    service.sync().await.unwrap();
    assert!(!output.join("Projects/demo-2").exists());
    assert!(!output.join("Projects/demo-3").exists());
    assert_eq!(
        std::fs::read_to_string(output.join("Projects/demo/extra.md")).unwrap(),
        "Retained extra"
    );
    service.sync().await.unwrap();
}

#[tokio::test]
async fn project_merge_preserves_archived_issues_and_milestones() {
    let f = Fixture::new().await;
    let root = tempfile::tempdir().unwrap();
    let remote = "git@example.com:team/demo.git";
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .arg("init")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(["remote", "add", "origin", remote])
            .output()
            .unwrap()
            .status
            .success()
    );
    let duplicate = f
        .service
        .execute(
            json!({"command":"project.register","name":"demo","root":root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    // Model legacy duplicate records created before remote-based registration.
    let mut conn = super::incremental::connection(&f).await;
    sqlx::query("UPDATE projects SET data=json_set(data,'$.remote',?) WHERE id IN (?,?)")
        .bind(remote)
        .bind(&f.project)
        .bind(duplicate["id"].as_str().unwrap())
        .execute(&mut conn)
        .await
        .unwrap();

    let mut ids = Vec::new();
    for project in [json!(f.project), duplicate["id"].clone()] {
        for kind in ["issue", "milestone"] {
            let entity = f.service.execute(json!({"command":format!("{kind}.create"),"project":project,"title":"Archived","goal":"Preserve"}), WriteOptions::default()).await.unwrap().result;
            let id = entity["id"].as_str().unwrap();
            for action in ["cancel", "archive"] {
                f.service
                    .execute(
                        json!({"command":format!("{kind}.{action}"),kind:id}),
                        WriteOptions::default(),
                    )
                    .await
                    .unwrap();
            }
            ids.push((kind, id.to_owned()));
        }
    }
    f.service
        .execute(
            json!({"command":"project.merge","project":f.project,"source":duplicate["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    for (kind, id) in ids {
        let path = if kind == "issue" {
            &state
                .issues
                .iter()
                .find(|v| v.id == id)
                .unwrap()
                .document_path
        } else {
            &state
                .milestones
                .iter()
                .find(|v| v.id == id)
                .unwrap()
                .document_path
        };
        assert!(path.contains("/Archived/"), "{path}");
        assert!(f.service.config().output_dir().join(path).exists());
        assert!(
            !f.service
                .config()
                .output_dir()
                .join(path.replace("/Archived/", "/"))
                .exists()
        );
    }
    f.service.sync().await.unwrap();
}

#[tokio::test]
async fn project_merge_preserves_numeric_original_name() {
    for reverse in [false, true] {
        let f = Fixture::new().await;
        let root = tempfile::tempdir().unwrap();
        for args in [vec!["init"], vec!["remote", "add", "origin", "same"]] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(root.path())
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let original = f.service.execute(json!({"command":"project.register","name":"release-2026","root":root.path().join("old")}),WriteOptions::default()).await.unwrap().result;
        let duplicate = f
            .service
            .execute(
                json!({"command":"project.register","name":"release-2026","root":root.path()}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        f.service.execute(json!({"command":"project.register","name":"release","root":root.path().join("unrelated")}),WriteOptions::default()).await.unwrap();
        let mut conn = super::incremental::connection(&f).await;
        sqlx::query("UPDATE projects SET data=json_set(data,'$.remote','same') WHERE id IN (?,?)")
            .bind(original["id"].as_str().unwrap())
            .bind(duplicate["id"].as_str().unwrap())
            .execute(&mut conn)
            .await
            .unwrap();
        let (a, b) = if reverse {
            (&duplicate, &original)
        } else {
            (&original, &duplicate)
        };
        let merged = f
            .service
            .execute(
                json!({"command":"project.merge","project":a["id"],"source":b["id"]}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(merged.result["key"], "release-2026");
        assert_eq!(merged.result["id"], original["id"]);
        assert!(
            !f.service
                .config()
                .output_dir()
                .join("Projects/release-2026-2")
                .exists()
        );
    }
}
