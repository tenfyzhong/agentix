use super::*;

#[tokio::test]
async fn conversation_sync_filters_legacy_context_and_keeps_real_user_requests() {
    for format in ["markdown", "obsidian"] {
        let f = Fixture::new(format).await;
        let context = "# AGENTS.md instructions\n<INSTRUCTIONS>Injected rules</INSTRUCTIONS><environment_context>cwd: /work</environment_context>";
        let mut job =
            serde_json::to_value(&f.service.store().snapshot().await.unwrap().jobs[0]).unwrap();
        job["prompt"] = json!(context);
        job["conversation"] = json!([
            {"id":"context","session_id":"s","role":"user","text":context,"recorded_at":1},
            {"id":"u","session_id":"s","role":"user","text":"Please update AGENTS.md.","recorded_at":2},
            {"id":"a1","session_id":"s","role":"assistant","text":"First paragraph.\n","recorded_at":3},
            {"id":"a2","session_id":"s","role":"assistant","text":"Second paragraph.","recorded_at":4}
        ]);
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(&f.service.config().storage.path),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE jobs SET data = ? WHERE id = ?")
            .bind(job.to_string())
            .bind(&f.job)
            .execute(&pool)
            .await
            .unwrap();
        f.service.sync().await.unwrap();
        let body = f.service.job_markdown(&f.job).await.unwrap();
        assert!(!body.contains("Injected rules"));
        assert!(!body.contains("cwd: /work"));
        assert!(body.contains("Please update AGENTS.md."));
        assert_eq!(body.matches("### Agent output").count(), 1);
        assert!(body.contains("> First paragraph.\n>\n> Second paragraph."));
        let path = f
            .service
            .config()
            .output_dir()
            .join(job["document_path"].as_str().unwrap());
        let doc = std::fs::read_to_string(&path).unwrap();
        f.service.sync().await.unwrap();
        assert_eq!(doc, std::fs::read_to_string(path).unwrap());
    }
}

#[tokio::test]
async fn conversation_selects_new_job_in_same_second_and_can_target_previous_job() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Previous work").await;
    let claim = f.start(&task, "shared").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let options = WriteOptions {
        session_ref: Some("shared".into()),
        ..WriteOptions::default()
    };
    let next = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Next request"}),
            options.clone(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let request = json!({"command":"session.record","session":"shared","messages":[{"id":"next","role":"user","text":"Next prompt"}]});
    let result = f.service.execute(request, options.clone()).await.unwrap();
    assert_eq!(result.result["job_id"], next);
    let result = f.service.execute(json!({"command":"session.record","session":"shared","job":f.job,"messages":[{"id":"previous","role":"assistant","text":"Previous result"}]}),options).await.unwrap();
    assert_eq!(result.result["job_id"], f.job);
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.jobs[0].conversation[0].text, "Previous result");
    assert_eq!(state.jobs[1].conversation[0].text, "Next prompt");
}

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

#[tokio::test]
async fn job_conversation_records_text_after_delivery_without_changing_review_state() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Conversation").await;
    let claim = f.start(&task, "conversation").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let options = WriteOptions {
        session_ref: Some("conversation".into()),
        ..WriteOptions::default()
    };
    let request = json!({"command":"session.record","session":"conversation","messages":[
        {"id":"context","role":"user","text":"# AGENTS.md instructions\n<INSTRUCTIONS>Injected rules</INSTRUCTIONS><environment_context>cwd: /work</environment_context>"},
        {"id":"u1","role":"user","text":"Exact **prompt**"},
        {"id":"a1","role":"assistant","text":"Delivered.\n<!-- taskcli:notes:end -->"},
        {"id":"a2","role":"assistant","text":"Final **answer**.\n\n```rust\nfn main() {}\n```"}
    ]});
    f.service
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    let first = f.service.store().snapshot().await.unwrap().jobs[0].clone();
    f.service.execute(request, options.clone()).await.unwrap();
    let second = f.service.store().snapshot().await.unwrap().jobs[0].clone();
    assert_eq!(first, second);
    let job = serde_json::to_value(&second).unwrap();
    assert_eq!(job["status"], "PENDING_REVIEW");
    assert_eq!(job["prompt"], "Exact **prompt**");
    assert_eq!(job["conversation"].as_array().unwrap().len(), 3);
    let path = f.service.config().output_dir().join(&second.document_path);
    let doc = std::fs::read_to_string(&path).unwrap();
    assert!(doc.contains("## Conversation"));
    assert!(doc.contains("### Agent output"));
    assert_eq!(doc.matches("### Agent output").count(), 1);
    assert!(
        !doc.lines()
            .any(|line| line.starts_with("### ") && line.contains(" · "))
    );
    assert!(!doc.contains("Injected rules"));
    assert!(doc.contains("> Delivered.\n> <!-- taskcli:notes:end -->\n>\n> Final **answer**.\n>\n> ```rust\n> fn main() {}\n> ```"));
    std::fs::remove_file(&path).unwrap();
    Service::open(f.service.config().clone())
        .await
        .unwrap()
        .sync()
        .await
        .unwrap();
    assert_eq!(doc, std::fs::read_to_string(path).unwrap());
    assert!(f.service.execute(json!({"command":"session.record","session":"conversation","messages":[{"id":"tool","role":"tool","text":"private tool result"}]}),options.clone()).await.is_err());
    assert!(f.service.execute(json!({"command":"session.record","session":"another","job":f.job,"messages":[{"id":"forged","role":"assistant","text":"Forged"}]}),options).await.is_err());
}
