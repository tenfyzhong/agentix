use super::*;

fn board_properties(f: &Fixture) -> Value {
    let body = std::fs::read_to_string(
        f.service
            .config()
            .output_dir()
            .join("Projects/demo/Board.md"),
    )
    .unwrap();
    serde_yaml::from_str(
        body.strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0,
    )
    .unwrap()
}

#[tokio::test]
async fn obsidian_dashboard_is_a_scoped_read_only_table_with_project_links() {
    let f = Fixture::new("obsidian").await;
    let root = f.service.config().output_dir();
    let text = std::fs::read_to_string(root.join("Dashboard.base")).unwrap();
    let base: Value = serde_yaml::from_str(&text).unwrap();
    assert!(!root.join("Dashboard.md").exists());
    assert!(text.starts_with("# taskcli-generated: dashboard\n"));
    assert_eq!(base["formulas"]["name"], "link(file.path, note.name)");
    assert_eq!(base["formulas"]["status"], "note.status");
    assert_eq!(base["formulas"]["updated"], "date(note.updated_at)");
    assert_eq!(base["properties"]["formula.name"]["displayName"], "Name");
    assert_eq!(base["views"][0]["type"], "table");
    assert_eq!(
        base["views"][0]["order"],
        json!(["formula.name", "formula.status", "formula.updated"])
    );
    assert_eq!(
        base["views"][0]["sort"][0],
        json!({"column":"formula.updated","direction":"DESC"})
    );
    let filters = base["filters"]["and"].as_array().unwrap();
    for filter in [
        "file.inFolder(\"Tasks ☃/Projects\")",
        "file.name == \"Board\"",
        "file.ext == \"md\"",
        "file.hasTag(\"agent/project\")",
        "note.status == \"ACTIVE\"",
        "note[\"taskcli-generated\"] == true",
    ] {
        assert!(
            filters.contains(&json!(filter)),
            "missing {filter}: {filters:?}"
        );
    }
    assert!(!text.contains("Jobs/"));
    assert_eq!(
        f.service
            .store()
            .metadata("documents")
            .await
            .unwrap()
            .unwrap()["dashboard"],
        "Dashboard.base"
    );
}

#[tokio::test]
async fn dashboard_activity_comes_from_work_and_markdown_uses_a_compact_table() {
    let f = Fixture::new("markdown").await;
    let root = f.service.config().output_dir();
    let text = std::fs::read_to_string(root.join("Dashboard.md")).unwrap();
    assert!(text.contains("| Name | Status | Updated |"));
    assert!(text.contains("| [demo](Projects/demo/Board.md) | ACTIVE |"));
    assert!(!text.contains("## demo"));
    let original = board_properties(&f)["updated_at"].clone();
    f.clock.fetch_add(60, Ordering::SeqCst);
    f.service.sync().await.unwrap();
    assert_eq!(
        board_properties(&f)["updated_at"],
        original,
        "sync itself is not project activity"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Dashboard.md")).unwrap(),
        text
    );
    f.task("New activity").await;
    assert_ne!(board_properties(&f)["updated_at"], original);
    assert!(
        std::fs::read_to_string(root.join("Dashboard.md"))
            .unwrap()
            .contains("2026-09-05T00:01:00Z")
    );
}

#[tokio::test]
async fn dashboard_migration_protects_collisions_and_recovers_after_partial_publication() {
    let f = Fixture::new("markdown").await;
    let root = f.service.config().output_dir();
    let legacy = std::fs::read_to_string(root.join("Dashboard.md")).unwrap();
    std::fs::create_dir_all(f.service.config().documents.root.join(".obsidian")).unwrap();
    let mut config = f.service.config().clone();
    config.documents.format = agentix_task::DocumentFormat::Obsidian;
    let service = Service::new(config, f.service.store().clone()).unwrap();
    let base = root.join("Dashboard.base");
    std::fs::write(&base, "# My own Base\nviews: []\n").unwrap();
    let error = service.sync().await.unwrap_err();
    assert!(error.to_string().contains("unmanaged document"));
    assert_eq!(
        std::fs::read_to_string(&base).unwrap(),
        "# My own Base\nviews: []\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Dashboard.md")).unwrap(),
        legacy
    );
    std::fs::remove_file(&base).unwrap();
    std::fs::create_dir(&base).unwrap();
    assert!(service.sync().await.is_err());
    assert!(root.join("Dashboard.md").exists());
    std::fs::remove_dir(&base).unwrap();
    service.sync().await.unwrap();
    assert!(base.is_file());
    assert!(!root.join("Dashboard.md").exists());
    // Simulate publication before the projection manifest was committed.
    std::fs::write(root.join("Dashboard.md"), &legacy).unwrap();
    let mut paths = service
        .store()
        .metadata("documents")
        .await
        .unwrap()
        .unwrap();
    paths["dashboard"] = json!("Dashboard.md");
    service
        .store()
        .set_metadata("documents", &paths)
        .await
        .unwrap();
    service.sync().await.unwrap();
    assert!(!root.join("Dashboard.md").exists());
    // Switching back to portable Markdown removes only the registered Base.
    f.service.sync().await.unwrap();
    assert!(root.join("Dashboard.md").is_file());
    assert!(!base.exists());
}
