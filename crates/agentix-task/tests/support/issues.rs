use super::*;

#[tokio::test]
async fn issue_routing_exposes_current_association_and_bounded_same_project_requirements() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    let milestone = f.service.execute(json!({"command":"milestone.create","project":f.project,"title":"Launch","goal":"Ship login"}),WriteOptions::default()).await.unwrap().result;
    f.service
        .execute(
            json!({"command":"issue.update","issue":item["id"],"milestone":milestone["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    link(&f, &f.job, item["id"].as_str().unwrap()).await;
    let task = f.task("Login").await;
    f.start(&task, "worker").await;
    let context = f
        .service
        .routing_snapshot("worker", f.dir.path(), None, WriteOptions::default())
        .await
        .unwrap();
    assert_eq!(context["issue_id"], item["id"]);
    assert_eq!(context["issue"]["requirement"], item["requirement"]);
    assert_eq!(context["milestone"]["goal"], "Ship login");
    assert_eq!(context["issue_candidates"][0]["id"], item["id"]);
    assert_eq!(
        context["routing"]["candidates"][0]["job"]["issue_id"],
        item["id"]
    );
    assert!(context.get("inbox_id").is_none());
    assert!(context.get("inbox_todos").is_none());
    assert_eq!(context["routing"]["complete"], true);
}

#[tokio::test]
async fn issue_routing_bounds_candidates_and_reports_cancelled_work_for_its_session() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    link(&f, &f.job, item["id"].as_str().unwrap()).await;
    let task = f.task("Owned work").await;
    f.start(&task, "worker").await;
    f.service
        .execute(
            json!({"command":"issue.cancel","issue":item["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let heartbeat = f
        .service
        .execute(
            json!({"command":"session.heartbeat","session":"worker"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        heartbeat.result["issue_cancellations"][0],
        json!({"id":item["id"],"job_id":f.job})
    );
    for n in 0..33 {
        f.service
            .execute(
                json!({"command":"issue.create","project":f.project,"title":format!("Future {n}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let context = f
        .service
        .routing_snapshot("worker", f.dir.path(), None, WriteOptions::default())
        .await
        .unwrap();
    assert_eq!(context["issue_candidates"].as_array().unwrap().len(), 32);
    assert_eq!(context["routing"]["complete"], false);
    assert_eq!(
        context["issue_cancellations"][0],
        json!({"id":item["id"],"job_id":f.job})
    );
    assert!(
        !context["issue_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == item["id"])
    );
}

async fn issue(f: &Fixture) -> Value {
    f.service
        .execute(
            json!({"command":"issue.create","project":f.project,"title":"Login repair",
                "requirement":"Allow valid users to sign in.",
                "acceptance":"Valid credentials succeed and invalid credentials fail."}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result
}

async fn read_issue(f: &Fixture, id: &str) -> Value {
    serde_json::to_value(f.service.store().snapshot().await.unwrap()).unwrap()["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap()
        .clone()
}

async fn link(f: &Fixture, job: &str, id: &str) -> Value {
    f.service
        .execute(
            json!({"command":"job.update","job":job,"issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result
}

#[tokio::test]
async fn issue_association_checks_the_reread_revision_and_requirement_hash_atomically() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    for request in [
        json!({"command":"job.update","job":f.job,"issue":item["id"],"issue_revision":0,"issue_content_hash":item["content_hash"]}),
        json!({"command":"job.update","job":f.job,"issue":item["id"],"issue_revision":item["revision"],"issue_content_hash":"stale"}),
    ] {
        assert!(
            f.service
                .execute(request, WriteOptions::default())
                .await
                .is_err()
        );
        assert!(
            f.service
                .store()
                .snapshot()
                .await
                .unwrap()
                .jobs
                .iter()
                .find(|j| j.id == f.job)
                .unwrap()
                .issue_id
                .is_none()
        );
    }
    let linked=f.service.execute(json!({"command":"job.update","job":f.job,"issue":item["id"],"issue_revision":item["revision"],"issue_content_hash":item["content_hash"]}),WriteOptions::default()).await.unwrap();
    assert_eq!(linked.result["issue_id"], item["id"]);
}

#[tokio::test]
async fn issue_link_and_planning_preserve_todo_until_first_execution() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    assert_eq!(item["status"], "TODO");
    assert_eq!(link(&f, &f.job, id).await["issue_id"], id);
    let task = f.task("Implement login").await;
    let claimed = f.claim(&task, "worker").await;
    assert_eq!(read_issue(&f, id).await["status"], "TODO");
    assert!(
        f.service
            .execute(json!({"command":"task.start","task":task}), owner(&claimed))
            .await
            .is_err()
    );
    assert_eq!(read_issue(&f, id).await["status"], "TODO");
    f.plan(&task).await;
    f.service
        .execute(json!({"command":"task.start","task":task}), owner(&claimed))
        .await
        .unwrap();
    let active = read_issue(&f, id).await;
    assert_eq!(active["status"], "ACTIVE");
    assert!(active["started_at"].is_i64());
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claimed))
        .await
        .unwrap();
    assert_eq!(read_issue(&f, id).await["status"], "ACTIVE");
    f.approve().await;
    assert_eq!(read_issue(&f, id).await["status"], "ACTIVE");
}

#[tokio::test]
async fn issue_requires_every_job_terminal_and_human_acceptance() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    link(&f, &f.job, id).await;
    let second = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,
        "title":"Verify login","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(second["issue_id"], id);
    let task = f.task("Implement").await;
    let started = f.start(&task, "worker").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&started))
        .await
        .unwrap();
    let submit = json!({"command":"issue.submit","issue":id});
    assert!(
        f.service
            .execute(submit.clone(), WriteOptions::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("terminal")
    );
    f.approve().await;
    assert!(
        f.service
            .execute(submit.clone(), WriteOptions::default())
            .await
            .is_err()
    );
    f.service
        .execute(
            json!({"command":"job.cancel","job":second["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let pending = f
        .service
        .execute(submit, WriteOptions::default())
        .await
        .unwrap()
        .result;
    assert_eq!(pending["status"], "PENDING_REVIEW");
    let approve = json!({"command":"issue.approve","issue":id});
    assert!(
        f.service
            .execute(
                approve.clone(),
                WriteOptions {
                    actor_ref: "agent:codex".into(),
                    ..WriteOptions::default()
                }
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("human")
    );
    let done = f
        .service
        .execute(approve, WriteOptions::default())
        .await
        .unwrap()
        .result;
    assert_eq!(done["status"], "COMPLETED");
    assert!(done["completed_at"].is_i64());
}

#[tokio::test]
async fn issue_offline_delivery_accepts_title_only() {
    let f = Fixture::new().await;
    let blank = f
        .service
        .execute(
            json!({"command":"issue.create","project":f.project,"title":"Offline"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    f.service
        .execute(
            json!({"command":"issue.submit","issue":blank["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let item = issue(&f).await;
    f.service
        .execute(
            json!({"command":"issue.submit","issue":item["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.service
            .execute(
                json!({"command":"issue.approve","issue":item["id"]}),
                WriteOptions::default()
            )
            .await
            .unwrap()
            .result["status"],
        "COMPLETED"
    );
}

#[tokio::test]
async fn issue_cancel_revokes_leases_preserves_terminal_tasks_and_cancels_jobs() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    link(&f, &f.job, id).await;
    let finished = f.task("Finished result").await;
    let old = f.start(&finished, "old").await;
    f.service
        .execute(json!({"command":"task.done","task":finished}), owner(&old))
        .await
        .unwrap();
    // A separate round keeps the first Job's result and lease ownership distinct.
    f.approve().await;
    let second = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Next round","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let task = f
        .service
        .execute(
            json!({"command":"task.add","job":second["id"],"title":"Working"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let owned = f.start(&task, "working").await;
    let cancelled = f
        .service
        .execute(
            json!({"command":"issue.cancel","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(cancelled["status"], "CANCELLED");
    let state = f.service.store().snapshot().await.unwrap();
    assert!(state.leases.is_empty());
    assert_eq!(
        state.tasks[state.task_index(&finished).unwrap()]
            .status
            .to_string(),
        "DONE"
    );
    assert_eq!(
        state.tasks[state.task_index(&task).unwrap()]
            .status
            .to_string(),
        "CANCELLED"
    );
    assert_eq!(
        state.jobs[state.job_index(&f.job).unwrap()]
            .status
            .to_string(),
        "COMPLETED"
    );
    assert_eq!(
        state.jobs[state.job_index(second["id"].as_str().unwrap()).unwrap()]
            .status
            .to_string(),
        "CANCELLED"
    );
    assert!(
        f.service
            .execute(json!({"command":"task.done","task":task}), owner(&owned))
            .await
            .is_err()
    );
    f.service
        .execute(
            json!({"command":"issue.reopen","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(read_issue(&f, id).await["status"], "TODO");
    assert_eq!(
        f.service.store().snapshot().await.unwrap().tasks[state.task_index(&task).unwrap()]
            .status
            .to_string(),
        "CANCELLED"
    );
}

#[tokio::test]
async fn issue_association_is_single_project_scoped_and_revision_guarded() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    let linked = link(&f, &f.job, id).await;
    assert!(
        f.service
            .execute(
                json!({"command":"job.update","job":f.job,"issue":null}),
                WriteOptions {
                    expected_revision: Some(1),
                    ..WriteOptions::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        f.service
            .execute(
                json!({"command":"job.update","job":f.job,"issue":null}),
                WriteOptions {
                    expected_revision: linked["revision"].as_i64(),
                    ..WriteOptions::default()
                }
            )
            .await
            .unwrap()
            .result["issue_id"],
        Value::Null
    );
    let other = f
        .service
        .execute(
            json!({"command":"project.register","root":f.dir.path().join("other"),"name":"Other"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert!(
        f.service
            .execute(
                json!({"command":"job.create","project":other["id"],"title":"Foreign","issue":id}),
                WriteOptions::default()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("same Project")
    );
    assert!(
        f.service
            .execute(
                json!({"command":"job.update","job":f.job,"issue":[id]}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn issue_historical_job_link_does_not_start_issue() {
    let f = Fixture::new().await;
    let task = f.task("Independent delivery").await;
    let owned = f.start(&task, "worker").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&owned))
        .await
        .unwrap();
    f.approve().await;
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    link(&f, &f.job, id).await;
    assert_eq!(read_issue(&f, id).await["status"], "TODO");
}

#[tokio::test]
async fn issue_can_link_archived_historical_job_without_reopening_either() {
    let f = Fixture::new().await;
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
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    let linked = link(&f, &f.job, id).await;
    assert_eq!(linked["status"], "CANCELLED");
    assert!(linked["archived_at"].is_i64());
    assert_eq!(read_issue(&f, id).await["status"], "TODO");
    assert_eq!(
        f.service.store().issue_detail(id).await.unwrap()["jobs"][0]["id"],
        f.job
    );
}

#[tokio::test]
async fn issue_late_link_of_executing_job_starts_issue() {
    let f = Fixture::new().await;
    let task = f.task("Independent work").await;
    f.start(&task, "worker").await;
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    link(&f, &f.job, id).await;
    assert_eq!(read_issue(&f, id).await["status"], "ACTIVE");
}

#[tokio::test]
async fn issue_reject_and_reopen_preserve_job_history() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    f.service
        .execute(
            json!({"command":"issue.submit","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"job.create","project":f.project,"title":"New round","issue":id}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    let rejected = f
        .service
        .execute(
            json!({"command":"issue.reject","issue":id,"reason":"Missing coverage"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(rejected["status"], "ACTIVE");
    assert_eq!(rejected["review_reason"], "Missing coverage");
    f.service
        .execute(
            json!({"command":"issue.submit","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"issue.approve","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"job.create","project":f.project,"title":"New round","issue":id}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn issue_milestone_delete_requires_unlink_and_same_project() {
    let f = Fixture::new().await;
    let milestone = f.service.execute(json!({"command":"milestone.create","project":f.project,"title":"MVP","goal":"Ship login","order":1,"due_date":"2026-10-20"}), WriteOptions::default()).await.unwrap().result;
    let item = issue(&f).await;
    let update = json!({"command":"issue.update","issue":item["id"],"milestone":milestone["id"]});
    let linked = f
        .service
        .execute(
            update.clone(),
            WriteOptions {
                expected_revision: Some(1),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap()
        .result;
    assert_eq!(linked["milestone_id"], milestone["id"]);
    assert!(
        f.service
            .execute(
                update,
                WriteOptions {
                    expected_revision: Some(1),
                    ..WriteOptions::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.service
            .execute(
                json!({"command":"milestone.delete","milestone":milestone["id"]}),
                WriteOptions::default()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("unlink")
    );
    f.service
        .execute(
            json!({"command":"issue.update","issue":item["id"],"milestone":null}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"milestone.delete","milestone":milestone["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        serde_json::to_value(f.service.store().snapshot().await.unwrap()).unwrap()["milestones"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn issue_parallel_first_starts_activate_once_and_cancel_invalidates_memory_sources() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    link(&f, &f.job, id).await;
    let second = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Parallel round","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let first_task = f.task("First").await;
    let second_task = f
        .service
        .execute(
            json!({"command":"task.add","job":second["id"],"title":"Second"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = f.claim(&first_task, "first").await;
    let second_owned = f.claim(&second_task, "second").await;
    f.plan(&first_task).await;
    f.plan(&second_task).await;
    let (a, b) = tokio::join!(
        f.service.execute(
            json!({"command":"task.start","task":first_task}),
            owner(&first)
        ),
        f.service.execute(
            json!({"command":"task.start","task":second_task}),
            owner(&second_owned)
        )
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(read_issue(&f, id).await["revision"], 2);
    f.service
        .execute(
            json!({"command":"issue.cancel","issue":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let cancellations = f
        .service
        .store()
        .memory_job_cancellations(0, 10)
        .await
        .unwrap();
    assert_eq!(cancellations.len(), 2);
    assert!(cancellations.iter().any(|c| c.job_id == f.job));
    assert!(
        cancellations
            .iter()
            .any(|c| c.job_id == second["id"].as_str().unwrap())
    );
    let events = f.service.store().events(None, 0, 100).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "job.cancelled")
            .count(),
        2
    );
}

#[tokio::test]
async fn issue_and_milestone_follow_project_rename_archive_and_delete() {
    let f = Fixture::new().await;
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let item = issue(&f).await;
    let id = item["id"].as_str().unwrap();
    let milestone = f
        .service
        .execute(
            json!({"command":"milestone.create","project":f.project,"title":"MVP"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert!(
        f.service
            .execute(
                json!({"command":"project.archive","project":f.project}),
                WriteOptions::default()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("Issues")
    );
    let output = f.service.config().output_dir();
    std::fs::rename(
        output.join("Projects/demo"),
        output.join("Projects/Renamed"),
    )
    .unwrap();
    f.service.sync().await.unwrap();
    assert!(
        read_issue(&f, id).await["document_path"]
            .as_str()
            .unwrap()
            .starts_with("Projects/Renamed/Issues/")
    );
    assert!(
        f.service
            .store()
            .milestone_record(milestone["id"].as_str().unwrap())
            .await
            .unwrap()
            .document_path
            .starts_with("Projects/Renamed/Milestones/")
    );
    f.service
        .execute(
            json!({"command":"issue.cancel","issue":id}),
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
    assert!(
        read_issue(&f, id).await["document_path"]
            .as_str()
            .unwrap()
            .starts_with("Archived Projects/Renamed/Issues/")
    );
    for entity in [id, milestone["id"].as_str().unwrap(), &f.job] {
        let note = f.service.obsidian_note(entity).await.unwrap();
        let text = std::fs::read_to_string(
            f.service
                .config()
                .documents
                .root
                .join(note["path"].as_str().unwrap()),
        )
        .unwrap();
        let props: Value = serde_yaml::from_str(text.split("---").nth(1).unwrap()).unwrap();
        assert_eq!(
            props["project"],
            "[[Archived Projects/Renamed/Board|Renamed]]"
        );
        assert_eq!(props["project_id"], f.project);
    }
    f.service
        .execute(
            json!({"command":"project.delete","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    assert!(state.issues.is_empty());
    assert!(state.milestones.is_empty());
}

#[tokio::test]
async fn issue_accepts_placeholder_content_but_rejects_invalid_creation_fields() {
    let f = Fixture::new().await;
    let item=f.service.execute(json!({"command":"issue.create","project":f.project,"title":"Unspecified",
        "requirement":"<!-- Describe the requirement. -->","acceptance":"<!-- Define acceptance criteria. -->"}),WriteOptions::default()).await.unwrap().result;
    assert!(
        f.service
            .execute(
                json!({"command":"issue.submit","issue":item["id"]}),
                WriteOptions::default()
            )
            .await
            .is_ok()
    );
    for field in ["requirement", "acceptance", "notes"] {
        let mut request = json!({"command":"issue.create","project":f.project,"title":"Invalid"});
        request[field] = json!(["unexpected"]);
        assert!(
            f.service
                .execute(request, WriteOptions::default())
                .await
                .is_err(),
            "{field}"
        );
    }
    for request in [
        json!({"command":"milestone.create","project":f.project,"title":"Bad order","order":"soon"}),
        json!({"command":"milestone.create","project":f.project,"title":"Bad date","due_date":"2026-02-30"}),
    ] {
        assert!(
            f.service
                .execute(request, WriteOptions::default())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn issue_milestone_association_rejects_foreign_project() {
    let f = Fixture::new().await;
    let item = issue(&f).await;
    let other = f
        .service
        .execute(
            json!({"command":"project.register","name":"Other","root":f.dir.path().join("other")}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let milestone = f
        .service
        .execute(
            json!({"command":"milestone.create","project":other["id"],"title":"Foreign"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert!(
        f.service
            .execute(
                json!({"command":"issue.update","issue":item["id"],"milestone":milestone["id"]}),
                WriteOptions::default()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("same Project")
    );
}

#[tokio::test]
async fn issue_cancellation_notices_prioritize_recent_cancellations() {
    let f = Fixture::new().await;
    let mut latest = Value::Null;
    for n in 0..33 {
        let item = f
            .service
            .store()
            .execute(
                json!({"command":"issue.create","project":f.project,"title":format!("Issue {n}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        f.service.store().execute(json!({"command":"job.create","project":f.project,"title":format!("Job {n}"),"issue":item["id"],"issue_revision":item["revision"],"issue_content_hash":item["content_hash"]}), WriteOptions {session_ref:Some("worker".into()), ..WriteOptions::default()}).await.unwrap();
        f.clock.fetch_add(1, Ordering::SeqCst);
        f.service
            .store()
            .execute(
                json!({"command":"issue.cancel","issue":item["id"]}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        latest = item["id"].clone();
    }
    let notices = f
        .service
        .store()
        .issue_cancellations("worker")
        .await
        .unwrap();
    assert_eq!(notices.as_array().unwrap().len(), 32);
    assert_eq!(notices[0]["id"], latest);
}

async fn milestone_command(f: &Fixture, id: &str, command: &str) -> Value {
    f.service
        .execute(
            json!({"command":format!("milestone.{command}"),"milestone":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result
}

#[tokio::test]
async fn milestone_lifecycle_is_manual_guarded_and_does_not_cancel_delivery() {
    let f = Fixture::new().await;
    let stage = f
        .service
        .execute(
            json!({"command":"milestone.create","project":f.project,"title":"Release"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(stage["status"], "PLANNED");
    let id = stage["id"].as_str().unwrap();
    let item = issue(&f).await;
    f.service
        .execute(
            json!({"command":"issue.update","issue":item["id"],"milestone":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    link(&f, &f.job, item["id"].as_str().unwrap()).await;
    let started = milestone_command(&f, id, "start").await;
    assert_eq!(started["status"], "ACTIVE");
    let error = f
        .service
        .execute(
            json!({"command":"milestone.complete","milestone":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unfinished Issues"), "{error}");
    assert!(
        f.service
            .execute(
                json!({"command":"milestone.archive","milestone":id}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    let cancelled = milestone_command(&f, id, "cancel").await;
    assert_eq!(cancelled["status"], "CANCELLED");
    assert_eq!(
        f.service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap()
            .status,
        agentix_task::IssueStatus::Todo
    );
    let snapshot = f.service.store().snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .jobs
            .iter()
            .find(|job| job.id == f.job)
            .unwrap()
            .status,
        agentix_task::JobStatus::Active
    );
    milestone_command(&f, id, "reopen").await;
    f.service
        .execute(
            json!({"command":"issue.cancel","issue":item["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(f.service.store().milestone_record(id).await.unwrap()).unwrap()["status"],
        "PLANNED"
    );
    let complete = milestone_command(&f, id, "complete").await;
    assert_eq!(complete["status"], "COMPLETED");
    let progress = f.service.store().milestone_detail(id).await.unwrap();
    assert_eq!(progress["progress"]["completed"], 0);
    assert_eq!(progress["progress"]["cancelled"], 1);
    assert!(
        f.service
            .execute(
                json!({"command":"milestone.cancel","milestone":id}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    assert!(
        f.service
            .execute(
                json!({"command":"milestone.reopen","milestone":id}),
                WriteOptions {
                    expected_revision: Some(1),
                    ..WriteOptions::default()
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn milestone_legacy_defaults_and_board_show_only_open_stages() {
    let f = Fixture::new().await;
    let mut stage = f
        .service
        .execute(
            json!({"command":"milestone.create","project":f.project,"title":"Release"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    for field in [
        "status",
        "started_at",
        "completed_at",
        "cancelled_at",
        "archived_at",
    ] {
        stage.as_object_mut().unwrap().remove(field);
    }
    let legacy: agentix_task::Milestone = serde_json::from_value(stage.clone()).unwrap();
    assert_eq!(serde_json::to_value(legacy).unwrap()["status"], "PLANNED");
    let root = f.service.config().output_dir();
    let issue_base: Value = serde_yaml::from_str(
        &std::fs::read_to_string(root.join("Projects/demo/Issue.base")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        issue_base["filters"],
        json!({"and":[format!("project_id == {}", json!(f.project))]})
    );
    let board = std::fs::read_to_string(root.join("Projects/demo/Milestone.base")).unwrap();
    let yaml: serde_json::Value = serde_yaml::from_str(&board).unwrap();
    assert!(
        yaml["views"][1]["filters"]["and"]
            .as_array()
            .unwrap()
            .contains(&json!({"or":["status == \"PLANNED\"", "status == \"ACTIVE\""]}))
    );
    assert!(
        yaml["views"][1]["order"]
            .as_array()
            .unwrap()
            .contains(&json!("status"))
    );
    let note =
        std::fs::read_to_string(root.join(stage["document_path"].as_str().unwrap())).unwrap();
    let properties: Value = serde_yaml::from_str(note.split("---").nth(1).unwrap()).unwrap();
    assert_eq!(properties["status"], "PLANNED");
}

#[tokio::test]
async fn milestone_schema_upgrade_backfills_status_and_rejects_legacy_writes() {
    let f = Fixture::new().await;
    let stage = f
        .service
        .execute(
            json!({"command":"milestone.create","project":f.project,"title":"Legacy"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let db = &f.service.config().storage.path;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(db))
            .await
            .unwrap();
    sqlx::raw_sql("DROP TRIGGER IF EXISTS milestone_status_insert; DROP TRIGGER IF EXISTS milestone_status_update; PRAGMA user_version=24; UPDATE milestones SET data=json_remove(data,'$.status','$.started_at','$.completed_at','$.cancelled_at','$.archived_at');").execute(&pool).await.unwrap();
    let upgraded = Store::open(db).await.unwrap();
    let raw: String = sqlx::query_scalar("SELECT data FROM milestones WHERE id=?")
        .bind(stage["id"].as_str().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap()["status"],
        "PLANNED"
    );
    assert!(
        sqlx::query("UPDATE milestones SET data=json_remove(data,'$.status')")
            .execute(&pool)
            .await
            .is_err()
    );
    let service = Service::new(f.service.config().clone(), upgraded).unwrap();
    service.sync().await.unwrap();
    let note = service
        .obsidian_note(stage["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(note["status"], "PLANNED");
}

#[tokio::test]
async fn milestone_existing_default_base_upgrades_and_preserves_later_filters() {
    let f = Fixture::new().await;
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Milestone.base");
    let mut base: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    base["filters"] = json!({"and":["file.hasTag(\"agent/milestone\")",format!("project_id == {}",json!(f.project))]});
    base["views"][1]["order"]
        .as_array_mut()
        .unwrap()
        .retain(|column| column != "status");
    std::fs::write(&path, serde_yaml::to_string(&base).unwrap()).unwrap();
    f.service.sync().await.unwrap();
    let upgraded: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(
        upgraded["views"][1]["filters"]["and"]
            .as_array()
            .unwrap()
            .contains(&json!({"or":["status == \"PLANNED\"", "status == \"ACTIVE\""]}))
    );
    let mut customized = upgraded;
    customized["views"][1]["filters"]["and"]
        .as_array_mut()
        .unwrap()
        .pop();
    std::fs::write(&path, serde_yaml::to_string(&customized).unwrap()).unwrap();
    f.service.sync().await.unwrap();
    let retained: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(retained, customized);
}

#[tokio::test]
async fn milestone_completion_checks_pending_review_and_refreshes_archive_links() {
    let f = Fixture::new().await;
    let stage = f
        .service
        .execute(
            json!({"command":"milestone.create","project":f.project,"title":"Release"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let id = stage["id"].as_str().unwrap();
    let item = issue(&f).await;
    f.service
        .execute(
            json!({"command":"issue.update","issue":item["id"],"milestone":id}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    for command in ["issue.start", "issue.submit"] {
        f.service
            .execute(
                json!({"command":command,"issue":item["id"]}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let error = f
            .service
            .execute(
                json!({"command":"milestone.complete","milestone":id}),
                WriteOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unfinished Issues"));
    }
    f.service
        .execute(
            json!({"command":"issue.approve","issue":item["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    milestone_command(&f, id, "start").await;
    let note = f.service.obsidian_note(id).await.unwrap();
    assert!(note["properties"]["started_at"].is_string());
    let complete = milestone_command(&f, id, "complete").await;
    let archived = milestone_command(&f, id, "archive").await;
    let root = f.service.config().output_dir();
    assert!(
        !root
            .join(complete["document_path"].as_str().unwrap())
            .exists()
    );
    assert!(
        root.join(archived["document_path"].as_str().unwrap())
            .exists()
    );
    let issue_note =
        std::fs::read_to_string(root.join(item["document_path"].as_str().unwrap())).unwrap();
    assert!(issue_note.contains("/Milestones/Archived/"));
    let restored = milestone_command(&f, id, "unarchive").await;
    assert_eq!(restored["document_path"], complete["document_path"]);
    assert!(
        !root
            .join(archived["document_path"].as_str().unwrap())
            .exists()
    );
    let issue_note =
        std::fs::read_to_string(root.join(item["document_path"].as_str().unwrap())).unwrap();
    assert!(!issue_note.contains("/Milestones/Archived/"));
}

#[tokio::test]
async fn milestone_events_include_status_without_loading_goal_text() {
    let f = Fixture::new().await;
    let stage = f.service.execute(json!({"command":"milestone.create","project":f.project,"title":"Release","goal":"Private goal text"}), WriteOptions::default()).await.unwrap().result;
    f.service
        .execute(
            json!({"command":"milestone.start","milestone":stage["id"]}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let events = f.service.store().events(None, 0, 100).await.unwrap();
    let event = events
        .iter()
        .find(|event| event.event_type == "milestone.start")
        .unwrap();
    assert_eq!(event.payload["status"], "ACTIVE");
    assert!(event.payload.get("goal").is_none());
}

#[tokio::test]
async fn milestone_cancelled_archive_requires_restore_before_reopening() {
    let f = Fixture::new().await;
    let stage = f
        .service
        .execute(
            json!({"command":"milestone.create","project":f.project,"title":"Release"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let id = stage["id"].as_str().unwrap();
    milestone_command(&f, id, "cancel").await;
    let archived = milestone_command(&f, id, "archive").await;
    assert_eq!(archived["status"], "CANCELLED");
    assert!(archived["archived_at"].is_i64());
    assert!(
        archived["document_path"]
            .as_str()
            .unwrap()
            .contains("/Milestones/Archived/")
    );
    assert!(
        f.service
            .execute(
                json!({"command":"milestone.reopen","milestone":id}),
                WriteOptions::default()
            )
            .await
            .is_err()
    );
    milestone_command(&f, id, "unarchive").await;
    let reopened = milestone_command(&f, id, "reopen").await;
    assert_eq!(reopened["status"], "PLANNED");
    assert!(reopened["cancelled_at"].is_null());
}

#[tokio::test]
async fn milestone_base_defaults_to_board_sorted_by_order_then_newest_creation() {
    let f = Fixture::new().await;
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Milestone.base");
    let base: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(base["views"][0]["name"], "Milestone board");
    for view in base["views"].as_array().unwrap() {
        assert_eq!(
            view["sort"],
            json!([
                {"column":"order","direction":"ASC"},
                {"column":"created_at","direction":"DESC"}
            ])
        );
    }
}

#[tokio::test]
async fn milestone_base_migrates_default_view_and_sort_without_losing_customizations() {
    let f = Fixture::new().await;
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Milestone.base");
    let mut base: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let views = base["views"].as_array_mut().unwrap();
    views.sort_by_key(|view| view["type"] == "kanban");
    for view in views.iter_mut() {
        view["sort"] =
            json!([{"column":"order","direction":"ASC"},{"column":"file.name","direction":"ASC"}]);
    }
    views[1]["columnWidth"] = json!(420);
    views[1]["filters"] = json!("order > 0");
    let custom =
        json!({"name":"Custom","type":"table","sort":[{"column":"due_date","direction":"DESC"}]});
    views.push(custom.clone());
    std::fs::write(&path, serde_yaml::to_string(&base).unwrap()).unwrap();
    f.service.sync().await.unwrap();
    let migrated: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(migrated["views"][0]["name"], "Milestone board");
    assert_eq!(migrated["views"][0]["columnWidth"], 420);
    assert_eq!(migrated["views"][0]["filters"], "order > 0");
    for view in &migrated["views"].as_array().unwrap()[..2] {
        assert_eq!(
            view["sort"],
            json!([{"column":"order","direction":"ASC"},{"column":"created_at","direction":"DESC"}])
        );
    }
    assert_eq!(migrated["views"][2], custom);
    let content = std::fs::read_to_string(&path).unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), content);
}

#[tokio::test]
async fn milestone_base_board_has_all_statuses_while_table_defaults_to_open() {
    let f = Fixture::new().await;
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Milestone.base");
    let base: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let views = base["views"].as_array().unwrap();
    assert!(views.iter().any(|view| view["type"] == "table"));
    let board = views
        .iter()
        .find(|view| view["type"] == "kanban")
        .expect("Milestone status board");
    assert_eq!(board["groupBy"]["property"], "note.status");
    assert_eq!(
        board["groupOrder"],
        json!(["PLANNED", "ACTIVE", "COMPLETED", "CANCELLED"])
    );
    assert!(
        board["order"]
            .as_array()
            .unwrap()
            .contains(&json!("formula.name"))
    );
    assert!(
        board["order"]
            .as_array()
            .unwrap()
            .contains(&json!("completed_issues"))
    );
    assert!(board.get("limit").is_none());
    assert!(board.get("filters").is_none());
    assert_eq!(
        base["filters"],
        json!({"and":["file.hasTag(\"agent/milestone\")",format!("project_id == {}",json!(f.project))]})
    );
    assert!(
        base["views"][1]["filters"]["and"]
            .as_array()
            .unwrap()
            .contains(&json!({"or":["status == \"PLANNED\"", "status == \"ACTIVE\""]}))
    );
}

#[tokio::test]
async fn milestone_base_upgrade_adds_board_once_and_preserves_custom_views() {
    let f = Fixture::new().await;
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Milestone.base");
    let mut base: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    base["views"]
        .as_array_mut()
        .unwrap()
        .retain(|view| view["type"] == "table");
    base["views"][0]["name"] = json!("Release schedule");
    base["views"][0]["columnSize"] = json!({"note.status":150});
    base["filters"] =
        json!({"and":[format!("project_id == {}",json!(f.project)),"status == \"COMPLETED\""]});
    std::fs::write(&path, serde_yaml::to_string(&base).unwrap()).unwrap();
    f.service.sync().await.unwrap();
    let mut upgraded: Value =
        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(upgraded["filters"], base["filters"]);
    assert_eq!(upgraded["views"][0], base["views"][0]);
    assert_eq!(upgraded["views"].as_array().unwrap().len(), 2);
    assert_eq!(upgraded["views"][1]["type"], "kanban");
    upgraded["views"][1]["name"] = json!("My stages");
    upgraded["views"][1]["columnWidth"] = json!(420);
    upgraded["views"][1]["filters"] = json!("order > 0");
    std::fs::write(&path, serde_yaml::to_string(&upgraded).unwrap()).unwrap();
    f.service.sync().await.unwrap();
    let retained: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(retained, upgraded);
}

#[tokio::test]
async fn milestone_base_upgrade_moves_default_global_status_filter_to_table() {
    let f = Fixture::new().await;
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/demo/Milestone.base");
    let mut base: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let open = json!({"or":["status == \"PLANNED\"", "status == \"ACTIVE\""]});
    base["filters"] = json!({"and":["file.hasTag(\"agent/milestone\")",format!("project_id == {}",json!(f.project)),open]});
    base["views"][1].as_object_mut().unwrap().remove("filters");
    std::fs::write(&path, serde_yaml::to_string(&base).unwrap()).unwrap();
    f.service.sync().await.unwrap();
    let upgraded: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(upgraded["filters"]["and"].as_array().unwrap().len(), 2);
    assert_eq!(upgraded["views"][1]["filters"], json!({"and":[open]}));
    assert!(
        upgraded["views"]
            .as_array()
            .unwrap()
            .iter()
            .any(|view| view["type"] == "kanban" && view.get("filters").is_none())
    );
}

#[tokio::test]
async fn milestone_base_renames_registered_plural_file_preserving_custom_views() {
    let f = Fixture::new().await;
    let root = f.service.config().output_dir();
    let old = root.join("Projects/demo/Milestones.base");
    let new = root.join("Projects/demo/Milestone.base");
    if new.exists() {
        std::fs::rename(&new, &old).unwrap();
    }
    let mut base: Value = serde_yaml::from_str(&std::fs::read_to_string(&old).unwrap()).unwrap();
    base["views"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"table","name":"View","filters":"order > 3"}));
    std::fs::write(&old, serde_yaml::to_string(&base).unwrap()).unwrap();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(f.dir.path().join("tasks.sqlite3")),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE document_registry SET path = 'Projects/demo/Milestones.base' WHERE key = ?",
    )
    .bind(format!("milestone-base:{}", f.project))
    .execute(&pool)
    .await
    .unwrap();
    f.service.sync().await.unwrap();
    assert!(new.exists(), "singular Milestone.base must be generated");
    assert!(!old.exists(), "registered plural file must be retired");
    let migrated: Value = serde_yaml::from_str(&std::fs::read_to_string(&new).unwrap()).unwrap();
    assert_eq!(migrated, base);
    let board = std::fs::read_to_string(root.join("Projects/demo/Board.md")).unwrap();
    assert!(board.contains("/Milestone.base"));
    assert!(!board.contains("/Milestones.base"));
    f.service.sync().await.unwrap();
    assert_eq!(
        serde_yaml::from_str::<Value>(&std::fs::read_to_string(new).unwrap()).unwrap(),
        base
    );
}

#[tokio::test]
async fn issue_type_is_validated_persisted_and_optional_for_legacy_issues() {
    let f = Fixture::new().await;
    for kind in [
        "feature",
        "bug",
        "enhancement",
        "documentation",
        "question",
        "refactor",
    ] {
        let item = f.service.execute(json!({"command":"issue.create","project":f.project,"title":"Typed issue","issue_type":kind}), WriteOptions::default()).await.unwrap().result;
        assert_eq!(item["issue_type"], kind);
        let stored = f
            .service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(&stored).unwrap()["issue_type"], kind);
        let document = f
            .service
            .config()
            .document_path(std::path::Path::new(&stored.document_path))
            .unwrap();
        let doc = std::fs::read_to_string(document).unwrap();
        let properties: Value = serde_yaml::from_str(doc.split("---").nth(1).unwrap()).unwrap();
        assert_eq!(properties["issue_type"], kind);
        let updated = f
            .service
            .execute(
                json!({"command":"issue.update","issue":item["id"],"issue_type":null}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        assert!(updated["issue_type"].is_null());
    }
    for invalid in [json!("unknown"), json!("task"), json!(42), json!(["bug"])] {
        assert!(f.service.execute(json!({"command":"issue.create","project":f.project,"title":"Invalid","issue_type":invalid}),WriteOptions::default()).await.is_err());
    }
    let item = issue(&f).await;
    assert!(item["issue_type"].is_null());
    let mut legacy = serde_json::to_value(
        f.service
            .store()
            .issue_record(item["id"].as_str().unwrap())
            .await
            .unwrap(),
    )
    .unwrap();
    legacy.as_object_mut().unwrap().remove("issue_type");
    assert!(serde_json::from_value::<agentix_task::Issue>(legacy).is_ok());
}
