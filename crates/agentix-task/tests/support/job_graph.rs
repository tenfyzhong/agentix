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
async fn job_graph_tracks_nodes_dependencies_and_renames() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
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
}

#[tokio::test]
async fn job_graph_includes_shared_cross_job_prerequisites_once() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
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
}

#[tokio::test]
async fn job_graph_escapes_task_labels_as_literal_text() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
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
}

#[tokio::test]
async fn job_graph_displays_all_seven_task_statuses_with_tasknotes_colors() {
    let settings: Value = serde_json::from_str(include_str!(
        "../../../../plugins/agent-task-manager/obsidian/tasknotes-settings.json"
    ))
    .unwrap();
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
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
}

#[tokio::test]
async fn job_graph_refreshes_status_and_links_after_rename_and_archive() {
    for format in ["obsidian", "markdown"] {
        let f = Fixture::new(format).await;
        let task = f.task("Ship & verify").await;
        let original_url = if format == "obsidian" {
            "Tasks ☃/Projects/demo/Tasks/260905-0001-Ship &amp; verify.md"
        } else {
            "../Tasks/260905-0001-Ship%20%26%20verify.md"
        };
        let original = job_document(&f).await;
        assert_node_link(graph(&original), &task, format, original_url);
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
        f.service
            .execute(
                json!({"command":"job.archive","job":f.job}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let archived = job_document(&f).await;
        let expected_url = if format == "obsidian" {
            "Tasks ☃/Projects/demo/Tasks/260905-0001-Revised.md"
        } else {
            "../../Tasks/260905-0001-Revised.md"
        };
        assert_node_link(graph(&archived), &task, format, expected_url);
        assert!(graph(&archived).contains("Revised · DONE"));
        assert!(!graph(&archived).contains(original_url));
        assert!(!graph(&archived).contains(":::status_IN_PROGRESS"));
        f.service.sync().await.unwrap();
        assert_eq!(job_document(&f).await, archived);
    }
}

fn assert_node_link(diagram: &str, task: &str, format: &str, target: &str) {
    if format == "obsidian" {
        assert!(diagram.contains(&format!(
            "{task}[\"<a class='internal-link' data-href='{target}' href='{target}'"
        )));
        assert!(!diagram.contains("obsidian://"));
    } else {
        assert!(diagram.contains(&format!("click {task} href \"{target}\" \"Open task\"")));
    }
}
