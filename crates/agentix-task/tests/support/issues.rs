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
async fn issue_offline_delivery_requires_authored_acceptance() {
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
    assert!(
        f.service
            .execute(
                json!({"command":"issue.submit","issue":blank["id"]}),
                WriteOptions::default()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("acceptance")
    );
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
async fn issue_rejects_placeholder_acceptance_and_invalid_creation_fields() {
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
            .is_err()
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
