use super::*;

#[tokio::test]
async fn job_prompt_survives_updates_sync_reopen_and_archive() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        let prompt = "Please preserve **this request**.\n\n```rust\nprintln!(\"hello\");\n```\n<!-- taskcli:goal:start -->\n<!-- taskcli:notes:end -->\n";
        let created = f.service.execute(
            json!({"command":"job.create","project":f.project,"title":"Original request","goal":"Acceptance","prompt":prompt}),
            WriteOptions::default(),
        ).await.unwrap();
        assert!(created.projection_pending.is_none(), "{created:?}");
        assert_eq!(created.result["prompt"], prompt);
        let id = created.result["id"].as_str().unwrap();
        let path = f
            .service
            .config()
            .output_dir()
            .join(created.result["document_path"].as_str().unwrap());
        let doc = std::fs::read_to_string(&path).unwrap();
        assert!(doc.contains("## Prompt\n"));
        assert!(!doc.lines().any(|line| line.starts_with("prompt:")));
        assert!(doc.contains("    Please preserve **this request**.\n\n    ```rust\n    println!(\"hello\");\n    ```\n    <!-- taskcli:goal:start -->\n    <!-- taskcli:notes:end -->\n"));
        std::fs::write(
            &path,
            doc.replace(
                "<!-- taskcli:notes:start -->",
                "<!-- taskcli:notes:start -->\nKeep authored notes.",
            ),
        )
        .unwrap();
        for request in [
            json!({"command":"job.update","job":id,"name":"Renamed","goal":"Updated acceptance"}),
            json!({"command":"job.cancel","job":id}),
            json!({"command":"job.archive","job":id}),
            json!({"command":"job.unarchive","job":id}),
        ] {
            let outcome = f
                .service
                .execute(request, WriteOptions::default())
                .await
                .unwrap();
            assert!(outcome.projection_pending.is_none(), "{outcome:?}");
            assert_eq!(outcome.result["prompt"], prompt);
        }
        let reopened = Service::open(f.service.config().clone()).await.unwrap();
        reopened.sync().await.unwrap();
        let state = reopened.store().snapshot().await.unwrap();
        let job = state.jobs.iter().find(|job| job.id == id).unwrap();
        assert_eq!(serde_json::to_value(job).unwrap()["prompt"], prompt);
        let doc = std::fs::read_to_string(reopened.config().output_dir().join(&job.document_path))
            .unwrap();
        assert!(doc.contains("## Prompt\n"));
        assert!(doc.contains("Keep authored notes."));
        let body = reopened.job_markdown(id).await.unwrap();
        assert!(body.contains("## Prompt\n"));
        assert!(body.contains("Please preserve **this request**."));
        assert!(body.contains("Updated acceptance"));
        let path = reopened.config().output_dir().join(&job.document_path);
        std::fs::remove_file(&path).unwrap();
        reopened.sync().await.unwrap();
        let restored = std::fs::read_to_string(path).unwrap();
        assert!(restored.contains("## Prompt\n"));
        assert!(restored.contains("    Please preserve **this request**."));
    }
}

#[tokio::test]
async fn job_prompt_can_be_updated_and_legacy_jobs_default_to_empty() {
    let f = Fixture::new("markdown").await;
    let state = f.service.store().snapshot().await.unwrap();
    let mut legacy = serde_json::to_value(&state.jobs[0]).unwrap();
    legacy.as_object_mut().unwrap().remove("prompt");
    let job: agentix_task::Job = serde_json::from_value(legacy).unwrap();
    assert_eq!(serde_json::to_value(job).unwrap()["prompt"], "");
    for prompt in ["A new original request", ""] {
        let result = f
            .service
            .execute(
                json!({"command":"job.update","job":f.job,"prompt":prompt}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.projection_pending.is_none());
        assert_eq!(result.result["prompt"], prompt);
        let body = f.service.job_markdown(&f.job).await.unwrap();
        assert_eq!(body.contains("## Prompt\n"), !prompt.is_empty());
    }
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(f.service.execute(json!({"command":"job.update","job":f.job,"name":"Renamed","prompt":"Cannot change closed Job"}), WriteOptions::default()).await.is_err());
}
