use super::*;

#[tokio::test]
async fn issue_workspace_launch_uses_the_existing_create_picker_and_resumes_one_linked_job() {
    let (dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item = write(
        &service,
        json!({"command":"issue.create","project":project,"title":"Login"}),
    )
    .await;
    let agent = Arc::new(FakeAgent {
        workspace_session_cwd: Some(dir.path().to_string_lossy().into_owned()),
        ..FakeAgent::with_sessions(vec![])
    });
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Start")).await;
    click(&engine, button(&last(&channel), "New workspace")).await;
    click(&engine, button(&last(&channel), "Create")).await;
    assert!(last(&channel).body.contains("Choose the agent to launch"));
    click(&engine, button(&last(&channel), "Codex")).await;
    assert!(
        agent
            .calls()
            .iter()
            .any(|call| call.starts_with("start:thr_mux_new:Taskix Issue request:"))
    );
    let detail = service
        .store()
        .issue_detail(item["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(detail["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(detail["jobs"][0]["issue_id"], item["id"]);
    assert_eq!(detail["issue"]["status"], "TODO");
}

#[tokio::test]
async fn issue_execution_rechecks_a_session_that_moved_to_another_project() {
    let (dir, service, task) = task_fixture().await;
    service
        .execute(
            json!({"command":"task.release","task":task,"reason":"Issue execution test"}),
            task_write_options(&service, &task).await,
        )
        .await
        .unwrap();
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let other = dir.path().join("other-project");
    std::fs::create_dir(&other).unwrap();
    write(
        &service,
        json!({"command":"project.register","root":other,"name":"Other"}),
    )
    .await;
    let item = write(
        &service,
        json!({"command":"issue.create","project":project,"title":"Login"}),
    )
    .await;
    let agent = Arc::new(FakeAgent::new());
    agent
        .sessions
        .lock()
        .unwrap()
        .retain(|s| s.id.as_str() == "thr_b");
    agent.sessions.lock().unwrap()[0].cwd = Some(dir.path().to_string_lossy().into_owned());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Start")).await;
    let token = button(&last(&channel), "thr_b");
    agent.sessions.lock().unwrap()[0].cwd = Some(other.to_string_lossy().into_owned());
    assert!(
        engine
            .handle_inbound(InboundEnvelope::action(
                "moved-session",
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                token
            ))
            .await
            .is_err()
    );
    assert!(
        service
            .store()
            .issue_detail(item["id"].as_str().unwrap())
            .await
            .unwrap()["jobs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!agent.calls().iter().any(|call| call.starts_with("start:")));
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Exercise both successful and cancelled deferred attachment lifecycles.
async fn issue_runtime_attachment_starts_only_after_success_and_cancel_fences_late_completion() {
    for cancel in [false, true] {
        let (dir, service, task) = task_fixture().await;
        service
            .execute(
                json!({"command":"task.release","task":task,"reason":"Issue execution test"}),
                task_write_options(&service, &task).await,
            )
            .await
            .unwrap();
        let project = service.store().snapshot().await.unwrap().projects[0]
            .id
            .clone();
        let item = write(
            &service,
            json!({"command":"issue.create","project":project,"title":"Login"}),
        )
        .await;
        let release = Arc::new(tokio::sync::Notify::new());
        let adapter = FakeAgent {
            history_gate: Some(release.clone()),
            ..FakeAgent::new()
        };
        adapter
            .sessions
            .lock()
            .unwrap()
            .retain(|s| s.id.as_str() == "thr_b");
        adapter.sessions.lock().unwrap()[0].cwd = Some(dir.path().to_string_lossy().into_owned());
        let agent = Arc::new(adapter);
        let channel = Arc::new(FakeChannel::default());
        let engine = Engine::new(
            agent.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        )
        .with_task_board(service.clone());
        engine
            .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
            .await
            .unwrap();
        click(&engine, button(&last(&channel), "Start")).await;
        let action = InboundEnvelope::action(
            uuid::Uuid::new_v4().to_string(),
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            button(&last(&channel), "thr_b"),
        );
        tokio::time::timeout(
            std::time::Duration::from_millis(300),
            engine.execute_work(agentix_core::EngineWork::Inbound(action)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            service
                .store()
                .issue_detail(item["id"].as_str().unwrap())
                .await
                .unwrap()["jobs"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        if cancel {
            engine
                .execute_work(agentix_core::EngineWork::Inbound(input("/cancel")))
                .await
                .unwrap();
        }
        release.notify_one();
        let completion = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            engine.next_pending_prompt(),
        )
        .await
        .unwrap();
        engine.execute_work(completion).await.unwrap();
        if !cancel {
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while !agent
                    .calls()
                    .iter()
                    .any(|call| call.starts_with("start:thr_b:Taskix Issue request:"))
                {
                    engine
                        .execute_work(engine.next_pending_prompt().await)
                        .await
                        .unwrap();
                }
            })
            .await
            .unwrap();
        }
        assert_eq!(
            service
                .store()
                .issue_detail(item["id"].as_str().unwrap())
                .await
                .unwrap()["jobs"]
                .as_array()
                .unwrap()
                .len(),
            usize::from(!cancel)
        );
        assert_eq!(
            agent
                .calls()
                .iter()
                .any(|call| call.starts_with("start:thr_b:Taskix Issue request:")),
            !cancel
        );
    }
}

#[tokio::test]
async fn pending_issue_edits_reject_changed_revision_generation_and_attachment_without_agent_input()
{
    for change in ["revision", "generation", "attachment"] {
        let (_dir, service, _) = task_fixture().await;
        let project = service.store().snapshot().await.unwrap().projects[0]
            .id
            .clone();
        let item=write(&service,json!({"command":"issue.create","project":project,"title":"Login","requirement":"Original"})).await;
        let agent = Arc::new(FakeAgent::new());
        let channel = Arc::new(FakeChannel::default());
        let engine = Engine::new(
            agent.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        )
        .with_task_board(service.clone());
        engine
            .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
            .await
            .unwrap();
        click(&engine, button(&last(&channel), "Edit requirement")).await;
        match change {
            "revision" => {
                write(&service,json!({"command":"issue.update","issue":item["id"],"notes":"Concurrent author"})).await;
            }
            "generation" => {
                agent
                    .generation
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            _ => engine.handle_inbound(input("/attach thr_a")).await.unwrap(),
        }
        assert!(
            engine
                .handle_inbound(input("Stale requirement"))
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            service
                .issue_content(item["id"].as_str().unwrap())
                .await
                .unwrap()
                .requirement,
            "Original"
        );
        assert!(!agent.calls().iter().any(|call| call.starts_with("start:")));
    }
}

#[tokio::test]
async fn readonly_issue_session_still_allows_milestone_edit_and_human_acceptance() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let stage = write(
        &service,
        json!({"command":"milestone.create","project":project,"title":"MVP"}),
    )
    .await;
    let item=write(&service,json!({"command":"issue.create","project":project,"title":"Login","requirement":"Deliver","acceptance":"Verified"})).await;
    let agent = Arc::new(FakeAgent {
        read_only: true,
        ..FakeAgent::new()
    });
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Milestone")).await;
    click(&engine, button(&last(&channel), "MVP")).await;
    assert_eq!(
        service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .milestone_id
            .as_deref(),
        stage["id"].as_str()
    );
    click(&engine, button(&last(&channel), "Submit")).await;
    click(&engine, button(&last(&channel), "Accept")).await;
    assert_eq!(
        service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .status,
        agentix_task::IssueStatus::Completed
    );
    assert!(!agent.calls().iter().any(|call| call.starts_with("start:")));
}

#[tokio::test]
async fn issue_new_round_preserves_the_sessions_existing_task_assignment() {
    let (_dir, service, _) = task_fixture().await;
    let state = service.store().snapshot().await.unwrap();
    let item = write(
        &service,
        json!({"command":"issue.create","project":state.projects[0].id,"title":"Login"}),
    )
    .await;
    write(
        &service,
        json!({"command":"job.update","job":state.jobs[0].id,"issue":item["id"]}),
    )
    .await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "New round")).await;
    let token = button(&last(&channel), "thr_a");
    assert!(
        engine
            .handle_inbound(InboundEnvelope::action(
                "new-round",
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                token
            ))
            .await
            .is_err()
    );
    assert_eq!(
        service
            .store()
            .issue_detail(item["id"].as_str().unwrap())
            .await
            .unwrap()["jobs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        service.store().routing_assignment("thr_a").await.unwrap()["job_id"],
        state.jobs[0].id
    );
}

#[tokio::test]
async fn issue_execution_session_picker_reaches_matches_after_the_first_six() {
    let (dir, service, task) = task_fixture().await;
    service
        .execute(
            json!({"command":"task.release","task":task,"reason":"Issue execution test"}),
            task_write_options(&service, &task).await,
        )
        .await
        .unwrap();
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item = write(
        &service,
        json!({"command":"issue.create","project":project,"title":"Login"}),
    )
    .await;
    let mut sessions = FakeAgent::new().sessions.lock().unwrap().clone();
    let template = sessions.remove(0);
    let sessions = (0..9)
        .map(|n| SessionSummary {
            id: SessionId::new(format!("thr_issue_{n}")),
            cwd: Some(dir.path().to_string_lossy().into_owned()),
            ..template.clone()
        })
        .collect();
    let agent = Arc::new(FakeAgent::with_sessions(sessions));
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Start")).await;
    click(&engine, button(&last(&channel), "Next sessions")).await;
    click(&engine, button(&last(&channel), "thr_issue_6")).await;
    assert!(
        agent
            .calls()
            .iter()
            .any(|call| call.starts_with("start:thr_issue_6:Taskix Issue request:"))
    );
    assert_eq!(
        service
            .store()
            .issue_detail(item["id"].as_str().unwrap())
            .await
            .unwrap()["jobs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn issue_execution_job_picker_reaches_multiple_active_rounds_without_creating_another() {
    let (_dir, service, task) = task_fixture().await;
    service
        .execute(
            json!({"command":"task.release","task":task,"reason":"Issue execution test"}),
            task_write_options(&service, &task).await,
        )
        .await
        .unwrap();
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item = write(
        &service,
        json!({"command":"issue.create","project":project,"title":"Login"}),
    )
    .await;
    let mut selected = None;
    for n in 0..9 {
        let job=write(&service,json!({"command":"job.create","project":project,"issue":item["id"],"title":format!("Round {n}")})).await;
        if n == 1 {
            selected = job["id"].as_str().map(str::to_owned);
        }
    }
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Start")).await;
    click(&engine, button(&last(&channel), "thr_a")).await;
    click(&engine, button(&last(&channel), "Next Jobs")).await;
    click(&engine, button(&last(&channel), "Round 1")).await;
    assert!(agent.calls().iter().any(|call| call.contains(&format!(
        "Continue ACTIVE Job {}",
        selected.as_deref().unwrap()
    ))));
    assert_eq!(
        service
            .store()
            .issue_detail(item["id"].as_str().unwrap())
            .await
            .unwrap()["jobs"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
}

#[tokio::test]
async fn issue_related_jobs_are_paginated_and_keep_archived_history() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item = write(
        &service,
        json!({"command":"issue.create","project":project,"title":"Login"}),
    )
    .await;
    for n in 0..9 {
        let job=write(&service,json!({"command":"job.create","project":project,"issue":item["id"],"title":format!("Round {n}")})).await;
        if n == 0 {
            write(&service, json!({"command":"job.cancel","job":job["id"]})).await;
            write(&service, json!({"command":"job.archive","job":job["id"]})).await;
        }
    }
    let (engine, channel) = engine(service).await;
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "All Jobs")).await;
    assert!(last(&channel).body.contains("Jobs (9)"));
    click(&engine, button(&last(&channel), "Next")).await;
    assert!(last(&channel).body.contains("Round 0"));
    assert!(last(&channel).body.contains("Archived"));
}

#[tokio::test]
async fn job_completion_notice_keeps_issue_active_and_only_reaches_its_bound_conversation() {
    let (_dir, service, task) = task_fixture().await;
    let state = service.store().snapshot().await.unwrap();
    let item=write(&service,json!({"command":"issue.create","project":state.projects[0].id,"title":"Login","requirement":"Deliver","acceptance":"Verified"})).await;
    write(
        &service,
        json!({"command":"job.update","job":state.jobs[0].id,"issue":item["id"]}),
    )
    .await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine
        .handle_inbound(inbound("chat-b", "/attach thr_b"))
        .await
        .unwrap();
    service
        .execute(
            json!({"command":"task.done","task":task}),
            task_write_options(&service, &task).await,
        )
        .await
        .unwrap();
    service
        .execute(
            json!({"command":"job.approve","job":state.jobs[0].id}),
            WriteOptions {
                session_ref: Some("thr_a".into()),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap();
    refresh_and_deliver_tasks(&engine).await.unwrap();
    let notices = channel
        .sent()
        .into_iter()
        .filter(|(_, view)| view.title == "Task update" && view.body.contains("job.completed"))
        .collect::<Vec<_>>();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].0.conversation_id, "chat-a");
    assert!(
        notices[0].1.body.contains("Issue remains Active"),
        "{:?}",
        notices[0].1
    );
    button(&notices[0].1, "New round");
    assert_eq!(
        service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .status,
        agentix_task::IssueStatus::Active
    );
}

#[tokio::test]
async fn issue_execution_offers_a_project_workspace_when_no_session_matches() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item = write(
        &service,
        json!({"command":"issue.create","project":project,"title":"Login"}),
    )
    .await;
    let agent = Arc::new(FakeAgent::with_sessions(vec![]));
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent,
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Start")).await;
    click(&engine, button(&last(&channel), "New workspace")).await;
    let preview = last(&channel);
    assert!(
        preview
            .body
            .contains(&service.store().project_result(&project).await.unwrap().root),
        "{preview:?}"
    );
    assert!(
        service
            .store()
            .issue_detail(item["id"].as_str().unwrap())
            .await
            .unwrap()["jobs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    engine.handle_inbound(input("/cancel")).await.unwrap();
}

#[tokio::test]
async fn issue_execution_attaches_the_selected_project_session_and_links_one_job() {
    let (dir, service, task) = task_fixture().await;
    service
        .execute(
            json!({"command":"task.release","task":task,"reason":"Issue execution test"}),
            task_write_options(&service, &task).await,
        )
        .await
        .unwrap();
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item=write(&service,json!({"command":"issue.create","project":project,"title":"Login","requirement":"Deliver login","acceptance":"Login works"})).await;
    let agent = Arc::new(FakeAgent::new());
    agent
        .sessions
        .lock()
        .unwrap()
        .retain(|s| s.id.as_str() == "thr_b");
    agent.sessions.lock().unwrap()[0].cwd = Some(dir.path().to_string_lossy().into_owned());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Start")).await;
    click(&engine, button(&last(&channel), "thr_b")).await;
    assert!(agent.calls().iter().any(|call| call == "attach:thr_b"));
    assert!(
        agent
            .calls()
            .iter()
            .any(|call| call.contains("Taskix Issue request:")),
        "{:?}",
        agent.calls()
    );
    let detail = service
        .store()
        .issue_detail(item["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(detail["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(detail["jobs"][0]["issue_id"], item["id"]);
    assert_eq!(detail["issue"]["status"], "TODO");
}

#[tokio::test]
async fn foreign_issue_reply_does_not_discard_the_owners_pending_edit() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item=write(&service,json!({"command":"issue.create","project":project,"title":"Login","requirement":"Original"})).await;
    let (engine, channel) = engine(service.clone()).await;
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Edit requirement")).await;
    assert!(
        engine
            .handle_inbound(InboundEnvelope::text(
                "foreign",
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "another-owner",
                "Forged"
            ))
            .await
            .is_err()
    );
    engine
        .handle_inbound(input("Updated requirement"))
        .await
        .unwrap();
    assert_eq!(
        service
            .issue_content(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .requirement,
        "Updated requirement"
    );
}

#[tokio::test]
async fn issue_help_is_available_without_attachment_and_has_no_inbox_commands() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/help")).await.unwrap();
    let view = last(&channel);
    for name in ["/issues", "/issue", "/milestones"] {
        assert!(view.body.contains(name), "{view:?}");
    }
    assert!(!view.body.contains("/inbox"));
}

#[tokio::test]
async fn issue_commands_create_template_requirements_without_an_agent_turn() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    let original = "Passkey login\nKeep the original requirement.\n- Mobile support";
    engine
        .handle_inbound(input(&format!(
            "/issue@agentix_bot new -title \"Passkey login\" {original}"
        )))
        .await
        .unwrap();
    let state = service.store().snapshot().await.unwrap();
    assert_eq!(state.issues.len(), 1);
    assert_eq!(state.issues[0].title, "Passkey login");
    assert_eq!(
        service
            .issue_content(&state.issues[0].id)
            .await
            .unwrap()
            .requirement,
        original
    );
    assert_eq!(state.issues[0].status, agentix_task::IssueStatus::Todo);
    assert!(last(&channel).body.contains(&state.issues[0].id));
    engine
        .handle_inbound(input(&format!("/issue {}", state.issues[0].id)))
        .await
        .unwrap();
    let detail = last(&channel);
    for label in ["Start", "Edit requirement", "Edit acceptance", "Milestone"] {
        button(&detail, label);
    }
    assert!(detail.body.contains("Keep the original requirement."));
}

#[tokio::test]
async fn issue_browsing_without_attachment_uses_a_project_picker() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/issues")).await.unwrap();
    let picker = last(&channel);
    assert_eq!(picker.title, "Choose project");
    click(&engine, button(&picker, "demo")).await;
    assert_eq!(last(&channel).title, "Project issues");
    assert!(last(&channel).body.contains("Issues (0)"));
}

#[tokio::test]
async fn issue_filter_paginates_after_sql_status_and_milestone_selection() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let milestone = write(
        &service,
        json!({"command":"milestone.create","project":project,"title":"MVP","goal":"Ship login"}),
    )
    .await;
    for n in 0..15 {
        let item=write(&service,json!({"command":"issue.create","project":project,"title":format!("Request {n}"),"milestone":if n<8 {milestone["id"].clone()} else {Value::Null}})).await;
        if n >= 8 {
            write(
                &service,
                json!({"command":"issue.cancel","issue":item["id"]}),
            )
            .await;
        }
    }
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine
        .handle_inbound(input(&format!(
            "/issues todo --milestone {}",
            milestone["id"].as_str().unwrap()
        )))
        .await
        .unwrap();
    let first = last(&channel);
    assert!(first.body.contains("Issues (8)"));
    assert_eq!(first.subtitle.as_deref(), Some("Page 1 / 2"));
    click(&engine, button(&first, "Next")).await;
    let second = last(&channel);
    assert_eq!(second.subtitle.as_deref(), Some("Page 2 / 2"));
    click(&engine, button(&second, "Request 7")).await;
    click(&engine, button(&last(&channel), "Project issues")).await;
    assert!(last(&channel).body.contains("Issues (8)"));
    engine.handle_inbound(input("/milestones")).await.unwrap();
    assert!(last(&channel).body.contains("MVP"));
}

#[tokio::test]
async fn issue_acceptance_is_separate_and_edit_cancel_does_not_reach_the_agent() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item=write(&service,json!({"command":"issue.create","project":project,"title":"Offline","requirement":"Deliver","acceptance":"Verified"})).await;
    let (engine, channel) = engine(service.clone()).await;
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Edit requirement")).await;
    engine.handle_inbound(input("/cancel")).await.unwrap();
    assert_eq!(
        service
            .issue_content(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .requirement,
        "Deliver"
    );
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Submit")).await;
    assert_eq!(
        service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .status,
        agentix_task::IssueStatus::PendingReview
    );
    let token = button(&last(&channel), "Accept");
    click(&engine, token.clone()).await;
    assert_eq!(
        service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .status,
        agentix_task::IssueStatus::Completed
    );
    assert!(
        engine
            .handle_inbound(InboundEnvelope::action(
                uuid::Uuid::new_v4().to_string(),
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                token
            ))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn issue_edit_and_session_rename_replace_each_others_reply_mode() {
    for issue_first in [true, false] {
        let (_dir, service, _) = task_fixture().await;
        let project = service.store().snapshot().await.unwrap().projects[0]
            .id
            .clone();
        let item = write(&service, json!({"command":"issue.create","project":project,"title":"Reply modes","requirement":"Original"})).await;
        let agent = Arc::new(FakeAgent::new());
        let channel = Arc::new(FakeChannel::default());
        let engine = Engine::new(
            agent.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        )
        .with_task_board(service.clone());
        engine.handle_inbound(input("/attach thr_a")).await.unwrap();
        engine
            .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
            .await
            .unwrap();
        let edit = button(&last(&channel), "Edit requirement");
        if issue_first {
            click(&engine, edit).await;
            engine.handle_inbound(input("/rename")).await.unwrap();
        } else {
            engine.handle_inbound(input("/rename")).await.unwrap();
            click(&engine, edit).await;
        }
        engine.handle_inbound(input("New content")).await.unwrap();
        let issue = service
            .issue_content(item["id"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            issue.requirement,
            if issue_first {
                "Original"
            } else {
                "New content"
            }
        );
        if !issue_first {
            engine
                .handle_inbound(input("Ordinary prompt"))
                .await
                .unwrap();
            assert!(
                !agent
                    .calls()
                    .iter()
                    .any(|call| call.contains("Rename(Some"))
            );
            assert!(
                agent
                    .calls()
                    .iter()
                    .any(|call| call.contains("Ordinary prompt"))
            );
        }
    }
}

#[tokio::test]
async fn issue_submission_notifies_bound_session_with_acceptance_controls() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item = write(&service, json!({"command":"issue.create","project":project,"title":"Review this","requirement":"Deliver","acceptance":"Verified"})).await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    service
        .execute(
            json!({"command":"issue.submit","issue":item["id"]}),
            WriteOptions {
                session_ref: Some("thr_a".into()),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap();
    refresh_and_deliver_tasks(&engine).await.unwrap();
    let view = channel
        .sent()
        .into_iter()
        .map(|(_, v)| v)
        .find(|v| v.title == "Task update" && v.body.contains("issue.pending_review"))
        .expect("Issue submission notification");
    assert!(view.actions.iter().any(|a| a.label == "Accept"));
    assert!(view.actions.iter().any(|a| a.label == "Reject"));
}

#[tokio::test]
async fn deleted_issue_does_not_block_historical_job_notifications() {
    let (_dir, service, task) = task_fixture().await;
    let state = service.store().snapshot().await.unwrap();
    let project = &state.projects[0].id;
    let job = &state.jobs[0].id;
    let item = write(
        &service,
        json!({"command":"issue.create","project":project,"title":"Deleted requirement"}),
    )
    .await;
    write(&service, json!({"command":"job.update","job":job,"issue":item["id"],"issue_revision":item["revision"],"issue_content_hash":item["content_hash"]})).await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    service
        .execute(
            json!({"command":"task.done","task":task}),
            task_write_options(&service, &task).await,
        )
        .await
        .unwrap();
    write(
        &service,
        json!({"command":"project.delete","project":project}),
    )
    .await;
    refresh_and_deliver_tasks(&engine).await.unwrap();
    assert!(
        channel
            .sent()
            .iter()
            .any(|(_, v)| v.title == "Task update" && v.body.contains("job.pending_review"))
    );
    refresh_and_deliver_tasks(&engine).await.unwrap();
    assert_eq!(
        channel
            .sent()
            .iter()
            .filter(|(_, v)| v.title == "Task update" && v.body.contains("job.pending_review"))
            .count(),
        1
    );
}

#[tokio::test]
async fn directory_path_reply_replaces_pending_issue_edit() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    let item = write(&service, json!({"command":"issue.create","project":project,"title":"Directory reply","requirement":"Original"})).await;
    let (engine, channel) = engine(service.clone()).await;
    engine
        .handle_inbound(input(&format!("/issue {}", item["id"].as_str().unwrap())))
        .await
        .unwrap();
    click(&engine, button(&last(&channel), "Edit requirement")).await;
    engine.handle_inbound(input("/rmux")).await.unwrap();
    for label in ["+ Session", "Choose directory", "Enter path"] {
        click(&engine, button(&last(&channel), label)).await;
    }
    engine
        .handle_inbound(input("/custom/work tree"))
        .await
        .unwrap();
    assert_eq!(
        service
            .issue_content(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .requirement,
        "Original"
    );
    assert!(last(&channel).body.contains("/custom/work tree"));
}

#[tokio::test]
async fn issue_type_im_creation_and_selection_preserve_requirement() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine
        .handle_inbound(input(
            "/issue new -type refactor -title \"Login failure\" Login failure\nKeep details",
        ))
        .await
        .unwrap();
    let state = service.store().snapshot().await.unwrap();
    assert_eq!(state.issues[0].title, "Login failure");
    assert_eq!(
        serde_json::to_value(&state.issues[0]).unwrap()["issue_type"],
        "refactor"
    );
    assert_eq!(
        service
            .issue_content(&state.issues[0].id)
            .await
            .unwrap()
            .requirement,
        "Login failure\nKeep details"
    );
    click(&engine, button(&last(&channel), "Type")).await;
    click(&engine, button(&last(&channel), "enhancement")).await;
    let updated = service
        .store()
        .issue_record(&state.issues[0].id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(updated).unwrap()["issue_type"],
        "enhancement"
    );
    assert!(last(&channel).body.contains("enhancement"));
    click(&engine, button(&last(&channel), "Type")).await;
    click(&engine, button(&last(&channel), "None")).await;
    let updated = service
        .store()
        .issue_record(&state.issues[0].id)
        .await
        .unwrap();
    assert!(serde_json::to_value(updated).unwrap()["issue_type"].is_null());
}

#[tokio::test]
async fn issue_type_im_rejects_invalid_options_without_creating_issues() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    for command in [
        "/issue new -type invalid -title Title Description",
        "/issue new -type task -title Title Description",
        "/issue new -type",
        "/issue new -type bug",
    ] {
        engine.handle_inbound(input(command)).await.unwrap();
        assert_eq!(last(&channel).title, "Issue creation failed");
        assert!(!last(&channel).body.is_empty());
        assert!(service.store().snapshot().await.unwrap().issues.is_empty());
    }
}

#[tokio::test]
async fn issue_creation_requires_an_explicit_nonblank_title() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service.clone()).await;
    for command in [
        "/issue new Description only",
        "/issue new -type bug Description",
        "/issue new -title",
        "/issue new -title \"\" Description",
        "/issue new -title \"   \" Description",
        "/issue new -title \"Unclosed",
        "/issue new -title One -title Two",
        "/issue new --title Title",
        "/issue new -title Title --type bug",
        "/issue new -title -type bug",
        "/issue new -title \"Title\"suffix",
        "/issue new -title Title -type bug -type feature",
    ] {
        let request = input(command);
        let before = channel.sent().len();
        engine.handle_inbound(request.clone()).await.unwrap();
        assert_eq!(channel.sent().len(), before + 1, "{command}");
        assert_eq!(last(&channel).title, "Issue creation failed");
        assert!(!last(&channel).body.is_empty());
        engine.handle_inbound(request).await.unwrap();
        assert_eq!(
            channel.sent().len(),
            before + 1,
            "duplicate error: {command}"
        );
        assert!(service.store().snapshot().await.unwrap().issues.is_empty());
    }
}

#[tokio::test]
async fn issue_creation_preserves_explicit_title_and_optional_requirement_through_project_picker() {
    for (command, title, requirement) in [
        (
            "/issue new -title '  登录失败  ' -type bug First line\n  Second line",
            "登录失败",
            "First line\n  Second line",
        ),
        ("/issue new -title Title", "Title", ""),
        (
            "/issue new -title \"A title\" Description with 'quotes' and --flags",
            "A title",
            "Description with 'quotes' and --flags",
        ),
    ] {
        let (_dir, service, _) = task_fixture().await;
        let jobs_before = service.store().snapshot().await.unwrap().jobs.len();
        let (engine, channel) = engine(service.clone()).await;
        engine.handle_inbound(input(command)).await.unwrap();
        click(&engine, button(&last(&channel), "demo")).await;
        let state = service.store().snapshot().await.unwrap();
        assert_eq!(state.issues.len(), 1);
        assert_eq!(state.issues[0].title, title);
        assert_eq!(
            service
                .issue_content(&state.issues[0].id)
                .await
                .unwrap()
                .requirement,
            requirement
        );
        assert_eq!(state.jobs.len(), jobs_before);
    }
}

#[tokio::test]
async fn issue_creation_reports_archived_project_after_project_selection() {
    let (dir, service, _) = task_fixture().await;
    let root = dir.path().join("empty-project");
    std::fs::create_dir(&root).unwrap();
    let project = write(
        &service,
        json!({"command":"project.register","root":root,"name":"Empty project"}),
    )
    .await["id"]
        .clone();
    let (engine, channel) = engine(service.clone()).await;
    engine
        .handle_inbound(input("/issue new -title Example"))
        .await
        .unwrap();
    let token = button(&last(&channel), "Empty project");
    write(
        &service,
        json!({"command":"project.archive","project":project}),
    )
    .await;
    click(&engine, token).await;
    let view = last(&channel);
    assert_eq!(view.title, "Issue creation failed");
    assert!(view.body.to_lowercase().contains("archived"), "{view:?}");
    assert!(service.store().snapshot().await.unwrap().issues.is_empty());
}

#[tokio::test]
async fn issue_creation_reports_invalid_title_in_attached_conversation() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine
        .handle_inbound(input("/issue new -title '  '"))
        .await
        .unwrap();
    let view = last(&channel);
    assert_eq!(view.title, "Issue creation failed");
    assert!(view.body.contains("title"), "{view:?}");
    assert!(service.store().snapshot().await.unwrap().issues.is_empty());
}

#[tokio::test]
async fn issue_type_after_description_is_saved_through_project_picker() {
    for kind in [
        "feature",
        "bug",
        "enhancement",
        "documentation",
        "question",
        "refactor",
    ] {
        for (description, expected) in [
            (
                format!("First line\n  Second line -type {kind}"),
                "First line\n  Second line",
            ),
            (format!("First -type '{kind}' second"), "First second"),
        ] {
            let (_dir, service, _) = task_fixture().await;
            let (engine, channel) = engine(service.clone()).await;
            engine
                .handle_inbound(input(&format!(
                    "/issue new -title 'Login failure' {description}"
                )))
                .await
                .unwrap();
            click(&engine, button(&last(&channel), "demo")).await;
            let state = service.store().snapshot().await.unwrap();
            assert_eq!(
                serde_json::to_value(&state.issues[0]).unwrap()["issue_type"],
                kind
            );
            assert_eq!(
                service
                    .issue_content(&state.issues[0].id)
                    .await
                    .unwrap()
                    .requirement,
                expected
            );
        }
    }
}

#[tokio::test]
async fn issue_type_after_description_rejects_invalid_missing_and_duplicate_values() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    for content in [
        "Details -type unknown",
        "Details -type",
        "-type bug Details -type feature",
    ] {
        engine
            .handle_inbound(input(&format!("/issue new -title Title {content}")))
            .await
            .unwrap();
        assert_eq!(last(&channel).title, "Issue creation failed");
        assert!(service.store().snapshot().await.unwrap().issues.is_empty());
    }
}

#[tokio::test]
async fn issue_type_literals_in_description_are_preserved() {
    for (content, expected) in [
        (
            "Document '-type bug' literally",
            "Document '-type bug' literally",
        ),
        ("-- -type bug is literal", "-type bug is literal"),
        ("Details -- -type bug", "Details -type bug"),
        ("Quote 'example'-type bug", "Quote 'example'-type bug"),
    ] {
        let (_dir, service, _) = task_fixture().await;
        let (engine, _channel) = engine(service.clone()).await;
        engine.handle_inbound(input("/attach thr_a")).await.unwrap();
        engine
            .handle_inbound(input(&format!("/issue new -title Title {content}")))
            .await
            .unwrap();
        let state = service.store().snapshot().await.unwrap();
        assert_eq!(state.issues.len(), 1);
        assert!(state.issues[0].issue_type.is_none());
        assert_eq!(
            service
                .issue_content(&state.issues[0].id)
                .await
                .unwrap()
                .requirement,
            expected
        );
    }
}

#[tokio::test]
async fn issue_type_help_explains_syntax_and_supported_values() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/help")).await.unwrap();
    let help = last(&channel).body;
    assert!(help.contains("-type <type>"));
    for kind in [
        "feature",
        "bug",
        "enhancement",
        "documentation",
        "question",
        "refactor",
    ] {
        assert!(help.contains(kind), "missing {kind}");
    }
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine.handle_inbound(input("/issues")).await.unwrap();
    assert!(last(&channel).body.contains("-type <type>"));
}
