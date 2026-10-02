use super::*;

fn human(revision: i64) -> WriteOptions {
    WriteOptions {
        actor_ref: "user:obsidian".into(),
        expected_revision: Some(revision),
        ..WriteOptions::default()
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Verify the atomic cascade, unrelated ownership and emitted events together.
async fn obsidian_job_completion_finishes_unfinished_tasks_and_revokes_leases() {
    let f = Fixture::new().await;
    let mut unfinished = Vec::new();
    for status in ["TODO", "BLOCKED", "WAITING_USER"] {
        unfinished.push(task_in_state(&f, status).await);
    }
    for status in ["DONE", "FAILED", "CANCELLED"] {
        task_in_state(&f, status).await;
    }
    unfinished.push(task_in_state(&f, "IN_PROGRESS").await);
    let before = f.service.store().snapshot().await.unwrap();
    let old_lease = before.leases[0].clone();
    let unrelated = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Unrelated"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_task = f
        .service
        .execute(
            json!({"command":"task.add","job":unrelated,"title":"Keep active"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let other_claim = f
        .claim(other_task["id"].as_str().unwrap(), "unrelated-owner")
        .await;
    let revision = before.jobs[0].revision;
    let result = f
        .service
        .execute(
            json!({"command":"job.approve","job":f.job}),
            human(revision),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(result["status"], "COMPLETED");
    assert!(result["completed_at"].is_number());
    let after = f.service.store().snapshot().await.unwrap();
    assert_eq!(after.leases.len(), 1);
    assert_eq!(after.leases[0].token, other_claim["lease"]["token"]);
    for task in after.tasks.iter().filter(|task| task.job_id == f.job) {
        let previous = before.tasks.iter().find(|old| old.id == task.id).unwrap();
        if previous.status.terminal() {
            assert_eq!(task, previous);
        } else {
            assert_eq!(task.status.to_string(), "DONE");
            assert_eq!(task.revision, previous.revision + 1);
            assert_eq!(task.completed_at, result["completed_at"].as_i64());
            assert!(task.phase.is_none() && task.reason.is_none());
            assert!(!task.system_block);
        }
    }
    let other = after
        .tasks
        .iter()
        .find(|task| task.id == other_task["id"])
        .unwrap();
    assert_eq!(other.status.to_string(), "IN_PROGRESS");
    let events = f
        .service
        .store()
        .events(Some(&f.job), 0, 1000)
        .await
        .unwrap();
    for id in unfinished {
        assert!(
            events
                .iter()
                .any(|event| event.task_id.as_deref() == Some(&id)
                    && event.event_type == "task.done"
                    && event.actor_ref == "user:obsidian")
        );
    }
    assert!(
        f.service
            .execute(
                json!({"command":"task.done","task":old_lease.task_id}),
                WriteOptions {
                    actor_ref: old_lease.executor_ref,
                    session_ref: Some(old_lease.session_ref),
                    lease_token: Some(old_lease.token),
                    ..WriteOptions::default()
                },
            )
            .await
            .is_err()
    );
    let note = f.service.obsidian_note(&f.job).await.unwrap();
    assert_eq!(note["properties"]["status"], "COMPLETED");
}

#[tokio::test]
async fn obsidian_job_completion_allows_an_empty_job() {
    let f = Fixture::new().await;
    let revision = f.service.store().snapshot().await.unwrap().jobs[0].revision;
    let result = f
        .service
        .execute(
            json!({"command":"job.approve","job":f.job}),
            human(revision),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(result["status"], "COMPLETED");
}

#[tokio::test]
async fn obsidian_job_completion_rejects_stale_revision_without_partial_writes() {
    let f = Fixture::new().await;
    let revision = f.service.store().snapshot().await.unwrap().jobs[0].revision;
    task_in_state(&f, "IN_PROGRESS").await;
    let before = f.service.store().snapshot().await.unwrap();
    assert!(
        f.service
            .execute(
                json!({"command":"job.approve","job":f.job}),
                human(revision),
            )
            .await
            .is_err()
    );
    let after = f.service.store().snapshot().await.unwrap();
    assert_eq!(before.tasks, after.tasks);
    assert_eq!(before.jobs, after.jobs);
    assert_eq!(before.leases, after.leases);
}

#[tokio::test]
async fn obsidian_can_finish_individual_tasks_without_an_agent_plan_or_lease() {
    for status in ["TODO", "BLOCKED", "WAITING_USER", "PLANNING", "IN_PROGRESS"] {
        let f = Fixture::new().await;
        let id = task_in_state(&f, status).await;
        let before = f.service.store().snapshot().await.unwrap();
        let task = before.tasks.iter().find(|task| task.id == id).unwrap();
        let result = f
            .service
            .execute(
                json!({"command":"task.done","task":id}),
                human(task.revision),
            )
            .await
            .unwrap()
            .result;
        assert_eq!(result["status"], "DONE", "{status}");
        assert!(result["completed_at"].is_number());
        assert!(result["lease"].is_null() && result["phase"].is_null());
    }
}

#[tokio::test]
async fn agents_cannot_use_obsidian_completion_to_bypass_lifecycle_guards() {
    for status in ["TODO", "WAITING_USER", "PLANNING", "IN_PROGRESS"] {
        let f = Fixture::new().await;
        let id = task_in_state(&f, status).await;
        let before = f.service.store().snapshot().await.unwrap();
        let options = WriteOptions {
            actor_ref: "agent:codex".into(),
            ..WriteOptions::default()
        };
        for request in [
            json!({"command":"task.done","task":id}),
            json!({"command":"job.approve","job":f.job}),
        ] {
            assert!(
                f.service.execute(request, options.clone()).await.is_err(),
                "{status}"
            );
        }
        let after = f.service.store().snapshot().await.unwrap();
        assert_eq!(before.tasks, after.tasks);
        assert_eq!(before.jobs, after.jobs);
    }
}
