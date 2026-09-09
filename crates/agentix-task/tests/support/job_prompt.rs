use super::*;

async fn job_with_conversation() -> Fixture {
    let f = Fixture::new().await;
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"prompt":"Original request"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let task = f.task("Conversation order").await;
    f.start(&task, "section-order").await;
    f.service
        .execute(
            json!({"command":"session.record","session":"section-order","job":f.job,"messages":[
                {"id":"u","role":"user","text":"Original request"},
                {"id":"a","role":"assistant","text":"Delivered."}
            ]}),
            WriteOptions {
                session_ref: Some("section-order".into()),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap();
    f
}

fn job_headings(document: &str) -> Vec<&str> {
    document
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .collect()
}

#[tokio::test]
async fn job_document_places_prompt_and_conversation_after_notes() {
    let f = job_with_conversation().await;
    let state = f.service.store().snapshot().await.unwrap();
    let path = f
        .service
        .config()
        .output_dir()
        .join(&state.jobs[0].document_path);
    let document = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        job_headings(&document),
        ["Goal", "Tasks", "Notes", "Prompt", "Conversation"],
        "Obsidian"
    );

    // Recreate the old section order with authored content before syncing.
    let (body, history) = document.split_once("\n## Prompt\n").unwrap();
    let (header, sections) = body.split_once("\n## Goal\n").unwrap();
    let legacy = format!("{header}\n## Prompt\n{history}\n## Goal\n{sections}")
        .replace("Ship it", "Authored goal.")
        .replace(
            "<!-- taskix:notes:start -->",
            "<!-- taskix:notes:start -->\nAuthored notes.",
        );
    std::fs::write(&path, legacy).unwrap();
    f.service.sync().await.unwrap();
    let synced = std::fs::read_to_string(&path).unwrap();
    assert_eq!(job_headings(&synced), job_headings(&document));
    assert!(synced.contains("Authored goal."));
    assert!(synced.contains("Authored notes."));
    assert!(synced.contains("    Original request"));
    assert!(synced.contains("> Delivered."));
    f.service.sync().await.unwrap();
    assert_eq!(synced, std::fs::read_to_string(path).unwrap());
}

#[tokio::test]
async fn job_markdown_places_prompt_and_conversation_after_notes() {
    let f = job_with_conversation().await;
    let body = f.service.job_markdown(&f.job).await.unwrap();
    assert_eq!(
        job_headings(&body),
        ["Goal", "Notes", "Prompt", "Conversation"],
        "Obsidian"
    );
}

#[tokio::test]
async fn conversation_sync_filters_legacy_context_and_keeps_real_user_requests() {
    let f = Fixture::new().await;
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

#[tokio::test]
async fn conversation_selects_new_job_in_same_second_and_can_target_previous_job() {
    let f = Fixture::new().await;
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
    let f = Fixture::new().await;
    let prompt = "Please preserve **this request**.\n\n```rust\nprintln!(\"hello\");\n```\n<!-- taskix:goal:start -->\n<!-- taskix:notes:end -->\n";
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
    assert!(doc.contains("    Please preserve **this request**.\n\n    ```rust\n    println!(\"hello\");\n    ```\n    <!-- taskix:goal:start -->\n    <!-- taskix:notes:end -->\n"));
    std::fs::write(
        &path,
        doc.replace(
            "<!-- taskix:notes:start -->",
            "<!-- taskix:notes:start -->\nKeep authored notes.",
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
    let doc =
        std::fs::read_to_string(reopened.config().output_dir().join(&job.document_path)).unwrap();
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

#[tokio::test]
async fn job_prompt_can_be_updated_and_legacy_jobs_default_to_empty() {
    let f = Fixture::new().await;
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
    let f = Fixture::new().await;
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
        {"id":"a1","role":"assistant","text":"Delivered.\n<!-- taskix:notes:end -->"},
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
    assert!(doc.contains("> Delivered.\n> <!-- taskix:notes:end -->\n>\n> Final **answer**.\n>\n> ```rust\n> fn main() {}\n> ```"));
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

#[tokio::test]
async fn conversation_pairs_each_prompt_with_its_own_agent_output() {
    let f = Fixture::new().await;
    let task = f.task("Multiple turns").await;
    f.start(&task, "turns").await;
    let options = WriteOptions {
        session_ref: Some("turns".into()),
        ..WriteOptions::default()
    };
    for messages in [
        json!([
            {"id":"u1","role":"user","text":"Original request"},
            {"id":"a1","role":"assistant","text":"First delivery"},
            {"id":"a2","role":"assistant","text":"First validation"}
        ]),
        json!([
            {"id":"u2","role":"user","text":"Add a followup\n<!-- taskix:notes:end -->"},
            {"id":"a3","role":"assistant","text":"Second delivery"}
        ]),
        json!([
            {"id":"u3","role":"user","text":"Original request"},
            {"id":"a4","role":"assistant","text":"Third delivery"}
        ]),
    ] {
        f.service
            .execute(
                json!({"command":"session.record","session":"turns","messages":messages}),
                options.clone(),
            )
            .await
            .unwrap();
    }
    let body = f.service.job_markdown(&f.job).await.unwrap();
    let first = body
        .split("### Turn 1")
        .nth(1)
        .unwrap()
        .split("### Turn 2")
        .next()
        .unwrap();
    assert!(first.contains("    Original request"));
    assert!(first.contains("> First delivery\n>\n> First validation"));
    assert!(!first.contains("Second delivery"));
    let second = body
        .split("### Turn 2")
        .nth(1)
        .unwrap()
        .split("### Turn 3")
        .next()
        .unwrap();
    assert!(second.contains("    Add a followup\n    <!-- taskix:notes:end -->"));
    assert!(second.contains("> Second delivery"));
    let third = body.split("### Turn 3").nth(1).unwrap();
    assert!(
        third.contains("    Original request"),
        "repeated wording is still a separate prompt"
    );
    assert!(third.contains("> Third delivery"));
    f.service.sync().await.unwrap();
    assert_eq!(body, f.service.job_markdown(&f.job).await.unwrap());
}

#[tokio::test]
async fn conversation_capture_adopts_followup_prompt_once_and_preserves_repeated_turns() {
    let f = Fixture::new().await;
    let task = f.task("Original delivery").await;
    let claim = f.start(&task, "followup-capture").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let options = WriteOptions {
        session_ref: Some("followup-capture".into()),
        ..WriteOptions::default()
    };
    f.service
        .execute(
            json!({"command":"job.followup","job":f.job,"prompt":"Supplement"}),
            options.clone(),
        )
        .await
        .unwrap();
    let capture = json!({"command":"session.record","session":"followup-capture","messages":[
        {"id":"host-user","role":"user","text":"Supplement"},
        {"id":"host-assistant","role":"assistant","text":"Followup delivery"}
    ]});
    f.service
        .execute(capture.clone(), options.clone())
        .await
        .unwrap();
    f.service.execute(capture, options.clone()).await.unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    let doc = std::fs::read_to_string(
        f.service
            .config()
            .output_dir()
            .join(&state.jobs[0].document_path),
    )
    .unwrap();
    let properties: Value = serde_yaml::from_str(
        doc.strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0,
    )
    .unwrap();
    assert!(
        properties["followup_at"].is_string(),
        "Job document timestamps use local ISO 8601"
    );
    let messages = &state.jobs[0].conversation;
    assert_eq!(
        messages.len(),
        2,
        "the explicit followup and host capture represent the same prompt"
    );
    assert_eq!(messages[0].id, "host-user");
    f.service
        .execute(
            json!({"command":"session.record","session":"followup-capture","messages":[
                {"id":"new-user","role":"user","text":"Supplement"},
                {"id":"new-assistant","role":"assistant","text":"Another delivery"}
            ]}),
            options,
        )
        .await
        .unwrap();
    assert_eq!(
        f.service.store().snapshot().await.unwrap().jobs[0]
            .conversation
            .len(),
        4
    );
}

#[tokio::test]
async fn conversation_history_replay_does_not_adopt_an_old_id_for_a_repeated_prompt() {
    let f = Fixture::new().await;
    let task = f.task("Original delivery").await;
    let claim = f.start(&task, "history").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let options = WriteOptions {
        session_ref: Some("history".into()),
        ..WriteOptions::default()
    };
    let mut messages = vec![
        json!({"id":"u1","role":"user","text":"Try again"}),
        json!({"id":"a1","role":"assistant","text":"Original delivery"}),
    ];
    f.service
        .execute(
            json!({"command":"session.record","session":"history","messages":messages}),
            options.clone(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"job.followup","job":f.job,"prompt":"Try again"}),
            options.clone(),
        )
        .await
        .unwrap();
    messages.extend([
        json!({"id":"u2","role":"user","text":"Try again"}),
        json!({"id":"a2","role":"assistant","text":"Revised delivery"}),
    ]);
    let request = json!({"command":"session.record","session":"history","messages":messages});
    for _ in 0..2 {
        f.service
            .execute(request.clone(), options.clone())
            .await
            .unwrap();
        let state = f.service.store().snapshot().await.unwrap();
        assert_eq!(
            state.jobs[0]
                .conversation
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["u1", "a1", "u2", "a2"]
        );
    }
}
