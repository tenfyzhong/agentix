use super::*;
use agentix_task::{Service, WriteOptions};
use serde_json::{Value, json};

#[path = "inbox.rs"]
mod inbox;

fn input(text: &str) -> InboundEnvelope {
    InboundEnvelope::text(
        uuid::Uuid::new_v4().to_string(),
        ConversationRef::new(ChannelKind::Telegram, "chat-a"),
        "owner",
        text,
    )
}

async fn write(service: &Service, request: Value) -> Value {
    service
        .execute(request, WriteOptions::default())
        .await
        .unwrap()
        .result
}

async fn engine(service: Arc<Service>) -> (Engine, Arc<FakeChannel>) {
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service);
    (engine, channel)
}

fn last(channel: &FakeChannel) -> OutboundView {
    channel.sent().last().unwrap().1.clone()
}

fn button(view: &OutboundView, label: &str) -> String {
    view.actions
        .iter()
        .find(|a| a.label == label)
        .unwrap_or_else(|| panic!("missing {label}: {view:?}"))
        .token
        .clone()
}

async fn click(engine: &Engine, token: String) {
    engine
        .handle_inbound(InboundEnvelope::action(
            uuid::Uuid::new_v4().to_string(),
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            token,
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn dashboard_project_board_task_job_roundtrip_renders_authored_markdown() {
    let (_dir, service, id) = task_fixture().await;
    service.execute(json!({"command":"plan.revise","task":id,"body":"## Implementation\n\n**Bold plan** with `code`\n\n- Test first"}), task_write_options(&service, &id).await).await.unwrap();
    let state = service.store().snapshot().await.unwrap();
    let job_path = service
        .config()
        .output_dir()
        .join(&state.jobs[0].document_path);
    let content = std::fs::read_to_string(&job_path)
        .unwrap()
        .replace("Ship", "**Ship safely**")
        .replace(
            "<!-- taskcli:notes:start -->",
            "<!-- taskcli:notes:start -->\n- Authored job note",
        );
    std::fs::write(job_path, content).unwrap();
    let before = service.store().snapshot().await.unwrap();
    let (engine, channel) = engine(service.clone()).await;
    engine
        .handle_inbound(input("/dashboard@agentix_bot"))
        .await
        .unwrap();
    let dashboard = last(&channel);
    assert_eq!(dashboard.title, "Dashboard");
    assert!(
        channel.menus.lock().unwrap().last().is_some_and(|menu| menu
            .commands
            .iter()
            .any(|command| command.name == "dashboard" && !command.contextual)),
        "Dashboard must establish its top-level menu before any attachment"
    );
    assert!(dashboard.body.contains("demo"));
    click(&engine, button(&dashboard, "demo")).await;
    let board = last(&channel);
    assert!(board.body.contains("IN_PROGRESS (1)"));
    click(&engine, button(&board, "Implement task board")).await;
    let task = last(&channel);
    assert!(task.body.contains("**Bold plan** with `code`"));
    assert!(!task.body.contains("taskcli-generated:"));
    click(&engine, button(&task, "Job")).await;
    let job = last(&channel);
    assert!(job.body.contains("**Ship safely**"));
    assert!(job.body.contains("- Authored job note"));
    assert!(!job.body.contains("<!-- taskcli:"));
    assert!(!job.body.contains("```mermaid"));
    click(&engine, button(&job, "Implement task board")).await;
    assert_eq!(last(&channel).body, task.body);
    assert_eq!(
        service.store().snapshot().await.unwrap(),
        before,
        "browsing must not mutate task state or Plan hashes"
    );
}

#[tokio::test]
async fn session_board_and_jobs_follow_attachment_and_keep_released_work() {
    let (_dir, service, id) = task_fixture().await;
    let state = service.store().snapshot().await.unwrap();
    write(
        &service,
        json!({"command":"task.add","job":state.jobs[0].id,"title":"Sibling"}),
    )
    .await;
    let unrelated = write(
        &service,
        json!({"command":"job.create","project":state.projects[0].id,"title":"Unrelated job"}),
    )
    .await;
    write(
        &service,
        json!({"command":"task.add","job":unrelated["id"],"title":"Unrelated task"}),
    )
    .await;
    let (engine, channel) = engine(service.clone()).await;
    for command in ["/board", "/jobs"] {
        engine.handle_inbound(input(command)).await.unwrap();
        assert!(last(&channel).body.contains("/attach"));
    }
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine.handle_inbound(input("/board")).await.unwrap();
    let board = last(&channel);
    for text in ["Current", "EXECUTING", "Sibling"] {
        assert!(board.body.contains(text));
    }
    assert!(!board.body.contains("Unrelated"));
    let old_token = button(&board, "Sibling");
    engine.handle_inbound(input("/jobs")).await.unwrap();
    assert!(!last(&channel).body.contains("Unrelated job"));
    click(&engine, button(&last(&channel), "Task board")).await;
    assert!(last(&channel).actions.iter().any(|a| a.label == "Sibling"));
    service
        .execute(
            json!({"command":"task.block","task":id,"reason":"Waiting for API"}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    engine.handle_inbound(input("/board")).await.unwrap();
    assert!(last(&channel).body.contains("BLOCKED (1)"));
    assert!(last(&channel).body.contains("Waiting for API"));
    engine.handle_inbound(input("/attach thr_b")).await.unwrap();
    let rejected = engine
        .handle_inbound(InboundEnvelope::action(
            "stale-board",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            old_token,
        ))
        .await;
    assert!(matches!(rejected, Err(EngineError::InvalidAction)));
    for command in ["/board", "/jobs"] {
        engine.handle_inbound(input(command)).await.unwrap();
        assert!(last(&channel).body.contains("No associated"));
        assert!(!last(&channel).body.contains(&id));
    }
}

#[tokio::test]
async fn dashboard_navigation_is_owner_and_conversation_scoped() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/dashboard")).await.unwrap();
    let token = button(&last(&channel), "demo");
    for (chat, owner) in [("chat-b", "owner"), ("chat-a", "intruder")] {
        let result = engine
            .handle_inbound(InboundEnvelope::action(
                uuid::Uuid::new_v4().to_string(),
                ConversationRef::new(ChannelKind::Telegram, chat),
                owner,
                token.clone(),
            ))
            .await;
        assert!(matches!(result, Err(EngineError::InvalidAction)));
    }
    click(&engine, token).await;
    assert!(last(&channel).body.contains("Implement task board"));
}

#[tokio::test]
async fn configured_menus_have_dashboard_and_attached_secondary_commands() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine.handle_inbound(input("/help")).await.unwrap();
    let help = last(&channel).body;
    for name in ["dashboard", "board", "jobs"] {
        assert!(help.contains(&format!("/{name}")));
    }
    assert!(!help.contains("/projects"));
    assert!(!help.contains("/sessionboard"));
    {
        let menus = channel.menus.lock().unwrap();
        let menu = menus.last().unwrap();
        assert_eq!(
            menu.commands
                .iter()
                .take(5)
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["sessions", "dashboard", "cancel", "rmux", "help"]
        );
        let secondary: Vec<_> = menu.commands.iter().skip(5).collect();
        assert!(secondary.iter().all(|c| c.contextual));
        assert!(secondary.windows(2).all(|pair| pair[0].name < pair[1].name));
        for name in ["board", "jobs"] {
            let command = menu.commands.iter().find(|c| c.name == name).unwrap();
            assert!(command.contextual);
        }
    }
    engine.handle_inbound(input("/detach")).await.unwrap();
    let menus = channel.menus.lock().unwrap();
    let menu = menus.last().unwrap();
    assert!(menu.commands.iter().any(|c| c.name == "dashboard"));
    assert!(
        !menu
            .commands
            .iter()
            .any(|c| matches!(c.name.as_str(), "board" | "jobs"))
    );
}

#[tokio::test]
async fn board_and_job_task_pagination_reaches_every_task() {
    let (_dir, service, _) = task_fixture().await;
    let state = service.store().snapshot().await.unwrap();
    for index in 0..22 {
        service.store().execute(json!({"command":"task.add","job":state.jobs[0].id,"title":format!("Queued {index}")}), WriteOptions::default()).await.unwrap();
    }
    service.sync().await.unwrap();
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    for command in ["/board", "/jobs"] {
        engine.handle_inbound(input(command)).await.unwrap();
        if command == "/jobs" {
            click(&engine, button(&last(&channel), "Task board")).await;
        }
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let view = last(&channel);
            for action in &view.actions {
                if action.label.starts_with("Queued ") || action.label == "Implement task board" {
                    seen.insert(action.label.clone());
                }
            }
            let Some(next) = view.actions.iter().find(|a| a.label == "Next") else {
                break;
            };
            click(&engine, next.token.clone()).await;
        }
        assert_eq!(seen.len(), 23);
        assert!(last(&channel).actions.iter().any(|a| a.label == "Previous"));
    }
}

#[tokio::test]
async fn long_markdown_details_are_paged_with_fences_and_job_navigation() {
    let (_dir, service, id) = task_fixture().await;
    let body = format!(
        "## Long plan\n\n```rust\n{}\n```\n\n**Last paragraph**",
        "let value = 123456789;\n".repeat(200)
    );
    service
        .execute(
            json!({"command":"plan.revise","task":id,"body":body}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    let (engine, channel) = engine(service).await;
    engine
        .handle_inbound(input(&format!("/task {id}")))
        .await
        .unwrap();
    let mut pages = 0;
    let mut last_found = false;
    loop {
        let view = last(&channel);
        pages += 1;
        assert!(
            view.body.len() < 2000,
            "each detail page leaves room for channel escaping"
        );
        assert_eq!(view.body.matches("```").count() % 2, 0);
        assert!(view.actions.iter().any(|a| a.label == "Job"));
        last_found |= view.body.contains("**Last paragraph**");
        let Some(next) = view.actions.iter().find(|a| a.label == "Next") else {
            break;
        };
        click(&engine, next.token.clone()).await;
    }
    assert!(pages > 1);
    assert!(last_found);
}

#[tokio::test]
async fn long_task_reason_is_paged_without_losing_the_plan() {
    let (_dir, service, id) = task_fixture().await;
    let reason = format!("{}Reason tail", "Waiting for review.\n".repeat(300));
    service
        .execute(
            json!({"command":"task.block","task":id,"reason":reason}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    let (engine, channel) = engine(service).await;
    engine
        .handle_inbound(input(&format!("/task {id}")))
        .await
        .unwrap();
    let mut content = String::new();
    loop {
        let view = last(&channel);
        assert!(
            view.body.len() < 2000,
            "reason must share the detail page budget"
        );
        content.push_str(&view.body);
        assert!(view.actions.iter().any(|a| a.label == "Job"));
        let Some(next) = view.actions.iter().find(|a| a.label == "Next") else {
            break;
        };
        click(&engine, next.token.clone()).await;
    }
    assert!(content.contains("Reason tail"));
    assert!(content.contains("# Plan"));
}

#[tokio::test]
async fn dashboard_and_session_jobs_paginate_and_exclude_archives() {
    let (dir, service, id) = task_fixture().await;
    service
        .execute(
            json!({"command":"task.block","task":id,"reason":"Keep session history"}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    let state = service.store().snapshot().await.unwrap();
    for index in 0..8 {
        let root = dir.path().join(format!("project-{index}"));
        std::fs::create_dir(&root).unwrap();
        let project = write(
            &service,
            json!({"command":"project.register","root":root,"name":format!("Project {index}")}),
        )
        .await;
        let job = write(&service, json!({"command":"job.create","project":state.projects[0].id,"title":format!("Job {index}")})).await;
        let task = write(
            &service,
            json!({"command":"task.add","job":job["id"],"title":format!("Task {index}")}),
        )
        .await;
        let task_id = task["id"].as_str().unwrap();
        write(&service, json!({"command":"task.claim","task":task_id,"executor":"agent:codex","session":"thr_a"})).await;
        service
            .execute(
                json!({"command":"task.block","task":task_id,"reason":"Keep session history"}),
                task_write_options(&service, task_id).await,
            )
            .await
            .unwrap();
        if index == 7 {
            write(
                &service,
                json!({"command":"project.archive","project":project["id"]}),
            )
            .await;
            write(&service, json!({"command":"job.cancel","job":job["id"]})).await;
            write(&service, json!({"command":"job.archive","job":job["id"]})).await;
        }
    }
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    for (command, prefix, initial) in [
        ("/dashboard", "Project ", "demo"),
        ("/jobs", "Job ", "Task board"),
    ] {
        engine.handle_inbound(input(command)).await.unwrap();
        let first = last(&channel).body;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let view = last(&channel);
            assert!(!view.body.contains(&format!("{prefix}7")));
            let entries: Vec<_> = view
                .actions
                .iter()
                .filter(|a| a.label.starts_with(prefix) || a.label == initial)
                .collect();
            assert!(entries.len() <= 6);
            seen.extend(entries.into_iter().map(|a| a.label.clone()));
            let Some(next) = view.actions.iter().find(|a| a.label == "Next") else {
                break;
            };
            click(&engine, next.token.clone()).await;
        }
        assert_eq!(seen.len(), 8);
        click(&engine, button(&last(&channel), "Previous")).await;
        assert_eq!(last(&channel).body, first);
    }
}

#[tokio::test]
async fn missing_documents_keep_metadata_and_bidirectional_navigation() {
    let (_dir, service, id) = task_fixture().await;
    let state = service.store().snapshot().await.unwrap();
    let output = service.config().output_dir();
    std::fs::remove_file(output.join(&state.plans[0].path)).unwrap();
    std::fs::remove_file(output.join(&state.jobs[0].document_path)).unwrap();
    let before = service.store().snapshot().await.unwrap();
    let (engine, channel) = engine(service.clone()).await;
    engine
        .handle_inbound(input(&format!("/task {id}")))
        .await
        .unwrap();
    assert!(
        last(&channel)
            .body
            .contains("Task document is unavailable.")
    );
    assert!(last(&channel).body.contains("EXECUTING"));
    click(&engine, button(&last(&channel), "Job")).await;
    assert!(last(&channel).body.contains("Job document is unavailable."));
    assert!(last(&channel).body.contains("Ship"));
    click(&engine, button(&last(&channel), "Implement task board")).await;
    assert!(
        last(&channel)
            .body
            .contains("Task document is unavailable.")
    );
    assert_eq!(service.store().snapshot().await.unwrap(), before);
}

#[tokio::test]
async fn long_task_and_job_titles_leave_room_for_paged_details() {
    let (_dir, service, id) = task_fixture().await;
    let state = service.store().snapshot().await.unwrap();
    service.execute(json!({"command":"task.update","task":id,"title":format!("{}Task title tail", "任务名称 ".repeat(400))}), task_write_options(&service, &id).await).await.unwrap();
    write(&service, json!({"command":"job.update","job":state.jobs[0].id,"title":format!("{}Job title tail", "Requirement ".repeat(400))})).await;
    let (engine, channel) = engine(service).await;
    engine
        .handle_inbound(input(&format!("/task {id}")))
        .await
        .unwrap();
    for expected in [
        vec!["Task title tail", "Job title tail", "# Plan"],
        vec!["Job title tail", "Ship"],
    ] {
        let mut content = String::new();
        loop {
            let view = last(&channel);
            assert!(
                view.title.chars().count() <= 60,
                "title must leave room for the body"
            );
            assert!(
                view.body.len() < 2000,
                "metadata must share the page budget"
            );
            content.push_str(&view.body);
            let Some(next) = view.actions.iter().find(|a| a.label == "Next") else {
                break;
            };
            click(&engine, next.token.clone()).await;
        }
        for text in expected {
            assert!(content.contains(text), "missing {text}");
        }
        if last(&channel).actions.iter().any(|a| a.label == "Job") {
            click(&engine, button(&last(&channel), "Job")).await;
        }
    }
}
