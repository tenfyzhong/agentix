use super::*;

async fn job_document(f: &Fixture) -> String {
    let state = f.service.store().snapshot().await.unwrap();
    let job = state.jobs.iter().find(|job| job.id == f.job).unwrap();
    std::fs::read_to_string(f.service.config().output_dir().join(&job.document_path)).unwrap()
}

fn graph(document: &str) -> &str {
    document
        .split_once("```mermaid\n")
        .expect("Job has a Mermaid dependency graph")
        .1
        .split_once("\n```")
        .unwrap()
        .0
}

#[tokio::test]
async fn job_graph_reduces_diamond_paths_through_local_tasks_only() {
    let f = Fixture::new().await;
    let other = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Upstream"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut external = Vec::new();
    for title in ["External root", "External child"] {
        external.push(
            f.service
                .execute(
                    json!({"command":"task.add","job":other,"title":title}),
                    WriteOptions::default(),
                )
                .await
                .unwrap()
                .result["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let root = &external[0];
    let child = &external[1];
    let left = f.task("Left branch").await;
    let right = f.task("Right branch").await;
    let end = f.task("Merge branches").await;
    for (task, dependency) in [
        (child, root),
        (&left, root),
        (&right, root),
        (&end, &left),
        (&end, &right),
        (&end, root),
        (&end, child),
    ] {
        f.service
            .execute(
                json!({"command":"task.depend","task":task,"dependency":dependency}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let document = job_document(&f).await;
    let diagram = graph(&document);
    assert!(!diagram.contains(&format!("{root} --> {end}")));
    for (from, to) in [
        (root, &left),
        (root, &right),
        (&left, &end),
        (&right, &end),
        (child, &end),
    ] {
        assert!(diagram.contains(&format!("{from} --> {to}")));
    }
    assert_eq!(diagram.matches(" --> ").count(), 5);
    for dependency in [&left, &right] {
        f.service
            .execute(
                json!({"command":"task.undepend","task":end,"dependency":dependency}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    // The external child's own edges are outside this Job's diagram, so
    // that invisible path must not hide the root's direct connection.
    let updated = job_document(&f).await;
    assert!(graph(&updated).contains(&format!("{root} --> {end}")));
    assert!(graph(&updated).contains(&format!("{child} --> {end}")));
}

#[tokio::test]
async fn job_graph_omits_transitive_edges_and_restores_them_when_paths_change() {
    let f = Fixture::new().await;
    let first = f.task("Locate performance issues").await;
    let second = f.task("Implement optimizations").await;
    let third = f.task("Validate optimizations").await;
    let independent = f.task("Independent prerequisite").await;
    let last = f.task("Supplement performance checks").await;
    for (task, dependency) in [
        (&second, &first),
        (&third, &second),
        (&last, &first),
        (&last, &second),
        (&last, &third),
        (&last, &independent),
    ] {
        f.service
            .execute(
                json!({"command":"task.depend","task":task,"dependency":dependency}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let document = job_document(&f).await;
    let diagram = graph(&document);
    assert!(!diagram.contains(&format!("{first} --> {last}")));
    assert!(!diagram.contains(&format!("{second} --> {last}")));
    for (from, to) in [
        (&first, &second),
        (&second, &third),
        (&third, &last),
        (&independent, &last),
    ] {
        assert!(diagram.contains(&format!("{from} --> {to}")));
    }
    assert_eq!(diagram.matches(" --> ").count(), 4);
    let state = f.service.store().snapshot().await.unwrap();
    let task = state.tasks.iter().find(|task| task.id == last).unwrap();
    assert_eq!(
        task.dependencies.len(),
        4,
        "stored execution gates stay intact"
    );
    f.service.sync().await.unwrap();
    assert_eq!(job_document(&f).await, document);
    f.service
        .execute(
            json!({"command":"task.undepend","task":second,"dependency":first}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let updated = job_document(&f).await;
    assert!(graph(&updated).contains(&format!("{first} --> {last}")));
    assert!(!graph(&updated).contains(&format!("{second} --> {last}")));
}

#[tokio::test]
async fn job_sync_removes_legacy_dependency_prose_and_preserves_authored_notes() {
    let f = Fixture::new().await;
    let prerequisite = f.task("Prerequisite").await;
    let dependent = f.task("Dependent").await;
    f.service
        .execute(
            json!({"command":"task.depend","task":dependent,"dependency":prerequisite}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    let path = f
        .service
        .config()
        .output_dir()
        .join(&state.jobs[0].document_path);
    let document = job_document(&f).await;
    std::fs::write(
        &path,
        document
            .replace(
                "\n## Notes",
                "\n  Dependencies: Legacy prerequisite\n\n## Notes",
            )
            .replace(
                "<!-- taskcli:notes:start -->",
                "<!-- taskcli:notes:start -->\nDependencies: Keep this authored note.",
            ),
    )
    .unwrap();
    f.service.sync().await.unwrap();
    let updated = job_document(&f).await;
    let tasks_section = updated
        .split_once("\n## Tasks\n")
        .unwrap()
        .1
        .split_once("\n## Notes\n")
        .unwrap()
        .0;
    assert!(!tasks_section.contains("Dependencies:"));
    assert!(graph(&updated).contains(&format!("{prerequisite} --> {dependent}")));
    assert!(tasks_section.contains("260905-0001-Prerequisite"));
    assert!(tasks_section.contains("260905-0002-Dependent"));
    assert!(updated.contains("Dependencies: Keep this authored note."));
    assert!(updated.contains("Ship it"));
    f.service.sync().await.unwrap();
    assert_eq!(job_document(&f).await, updated);
}

#[tokio::test]
async fn job_graph_tracks_nodes_dependencies_and_renames() {
    let f = Fixture::new().await;
    assert!(!job_document(&f).await.contains("```mermaid"));
    let first = f.task("Design").await;
    let second = f.task("Research").await;
    let dependent = f.task("Implement").await;
    let independent = f.task("Independent").await;
    for prerequisite in [&first, &second] {
        f.service
            .execute(
                json!({"command":"task.depend","task":dependent,"dependency":prerequisite}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let document = job_document(&f).await;
    let diagram = graph(&document);
    assert!(diagram.starts_with("flowchart TD\n"));
    for (id, name) in [
        (&first, "Design"),
        (&second, "Research"),
        (&dependent, "Implement"),
        (&independent, "Independent"),
    ] {
        assert!(diagram.contains(&format!("{name} · TODO")));
        assert!(diagram.contains(&format!("{id}[\"")));
        assert!(diagram.contains(":::status_TODO"));
    }
    assert!(diagram.contains(&format!("{first} --> {dependent}")));
    assert!(diagram.contains(&format!("{second} --> {dependent}")));
    assert_eq!(diagram.matches(" --> ").count(), 2);
    assert!(
        document.contains("260905-0001-Design"),
        "keep task note links"
    );
    let state = f.service.store().snapshot().await.unwrap();
    let path = f
        .service
        .config()
        .output_dir()
        .join(&state.jobs[0].document_path);
    std::fs::write(
        &path,
        document.replace(
            "<!-- taskcli:notes:start -->",
            "<!-- taskcli:notes:start -->\nKeep my notes.",
        ),
    )
    .unwrap();
    f.service
        .execute(
            json!({"command":"task.undepend","task":dependent,"dependency":first}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"task.update","task":second,"name":"Investigate"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let updated = job_document(&f).await;
    assert!(!graph(&updated).contains(&format!("{first} --> {dependent}")));
    assert!(graph(&updated).contains(&format!("{second} --> {dependent}")));
    assert!(graph(&updated).contains("Investigate · TODO"));
    assert!(updated.contains("Keep my notes."));
    assert!(updated.contains("Ship it"));
    f.service.sync().await.unwrap();
    assert_eq!(job_document(&f).await, updated);
}

#[tokio::test]
async fn job_graph_includes_shared_cross_job_prerequisites_once() {
    let f = Fixture::new().await;
    let other = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Upstream"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let external = f
        .service
        .execute(
            json!({"command":"task.add","job":other,"title":"API"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.service
        .execute(
            json!({"command":"task.add","job":other,"title":"Unrelated"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let first = f.task("Client").await;
    let second = f.task("Server").await;
    for task in [&first, &second] {
        f.service
            .execute(
                json!({"command":"task.depend","task":task,"dependency":external}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let document = job_document(&f).await;
    let diagram = graph(&document);
    assert_eq!(diagram.matches(&format!("{external}[\"")).count(), 1);
    assert!(diagram.contains("API (Job: Upstream)"));
    assert!(diagram.contains("API (Job: Upstream) · TODO"));
    assert!(diagram.contains(&format!("{external} --> {first}")));
    assert!(diagram.contains(&format!("{external} --> {second}")));
    assert!(!diagram.contains("Unrelated"));
    for task in [&first, &second] {
        f.service
            .execute(
                json!({"command":"task.undepend","task":task,"dependency":external}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    assert!(!graph(&job_document(&f).await).contains(&external));
}

#[tokio::test]
async fn job_graph_escapes_task_labels_as_literal_text() {
    let f = Fixture::new().await;
    let task = f.task("验证 & `code` end").await;
    let document = job_document(&f).await;
    let diagram = graph(&document);
    assert!(diagram.contains("验证 #38; #96;code#96; end · TODO"));
    assert!(diagram.contains(&format!("{task}[\"")));
    assert_eq!(
        diagram.lines().filter(|line| line.contains("[\"")).count(),
        1
    );
}

#[tokio::test]
async fn job_graph_displays_all_seven_task_statuses_with_tasknotes_colors() {
    let settings: Value = serde_json::from_str(include_str!(
        "../../../../plugins/agent-task-manager/obsidian/tasknotes-settings.json"
    ))
    .unwrap();
    let f = Fixture::new().await;
    populate_board_states(&f).await;
    let document = job_document(&f).await;
    let diagram = graph(&document);
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.tasks.len(), agentix_task::TaskStatus::ALL.len());
    for task in &state.tasks {
        let status = task.status.to_string();
        let color = settings["customStatuses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|setting| setting["value"] == status)
            .unwrap()["color"]
            .as_str()
            .unwrap();
        let node = diagram
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("{}[", task.id)))
            .unwrap();
        assert!(node.contains(&format!("{} · {status}", task.name)));
        assert!(node.ends_with(&format!(":::status_{status}")));
        assert!(diagram.contains(&format!(
            "classDef status_{status} fill:{color},stroke:{color},color:#1f2937"
        )));
    }
    assert_eq!(diagram.matches("classDef status_").count(), 7);
    assert!(!diagram.contains("status_PLANNING") && !diagram.contains("status_EXECUTING"));
}

#[tokio::test]
async fn job_graph_refreshes_status_and_links_after_rename_and_archive() {
    let f = Fixture::new().await;
    let task = f.task("Ship & verify").await;
    let original_url = "Tasks ☃/Projects/demo/Tasks/260905-0001-Ship &amp; verify.md";
    let original = job_document(&f).await;
    assert_node_link(graph(&original), &task, original_url);
    let claim = f.start(&task, "graph-status").await;
    assert!(graph(&job_document(&f).await).contains("Ship #38; verify · IN_PROGRESS"));
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"task.update","task":task,"name":"Revised"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.approve().await;
    f.service
        .execute(
            json!({"command":"job.archive","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let archived = job_document(&f).await;
    let expected_url = "Tasks ☃/Projects/demo/Tasks/260905-0001-Revised.md";
    assert_node_link(graph(&archived), &task, expected_url);
    assert!(graph(&archived).contains("Revised · DONE"));
    assert!(!graph(&archived).contains(original_url));
    assert!(!graph(&archived).contains(":::status_IN_PROGRESS"));
    f.service.sync().await.unwrap();
    assert_eq!(job_document(&f).await, archived);
}

fn assert_node_link(diagram: &str, task: &str, target: &str) {
    assert!(diagram.contains(&format!(
        "{task}[\"<a class='internal-link' data-href='{target}' href='{target}'"
    )));
    assert!(!diagram.contains("obsidian://"));
}
