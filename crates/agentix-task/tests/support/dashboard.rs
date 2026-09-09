use super::*;

#[tokio::test]
async fn recent_jobs_base_migrates_registered_legacy_path_after_safe_publication() {
    let f = Fixture::new().await;
    let root = f.service.config().output_dir();
    let old = root.join("Pending Review.base");
    let new = root.join("Recent Jobs.base");
    if new.exists() {
        std::fs::remove_file(&new).unwrap();
    }
    // Obsidian may have removed the generated comment from a registered Base.
    std::fs::write(&old, "views: []\n").unwrap();
    let mut paths = f
        .service
        .store()
        .metadata("documents")
        .await
        .unwrap()
        .unwrap();
    paths["pending-review"] = json!("Pending Review.base");
    f.service
        .store()
        .set_metadata("documents", &paths)
        .await
        .unwrap();
    std::fs::write(&new, "# User view\nviews: []\n").unwrap();
    assert!(
        f.service
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("unmanaged document")
    );
    assert!(
        old.exists(),
        "publish replacement before removing legacy file"
    );
    assert_eq!(
        std::fs::read_to_string(&new).unwrap(),
        "# User view\nviews: []\n"
    );
    std::fs::remove_file(&new).unwrap();
    f.service.sync().await.unwrap();
    assert!(new.exists());
    assert!(!old.exists());
}

#[tokio::test]
async fn recent_jobs_base_is_independent_scoped_and_safe_to_regenerate() {
    let f = Fixture::new().await;
    let root = f.service.config().output_dir();
    let path = root.join("Recent Jobs.base");
    let source = std::fs::read_to_string(&path).expect("independent review Base");
    let base: Value = serde_yaml::from_str(&source).unwrap();
    let filters = base["filters"]["and"].as_array().unwrap();
    for filter in [
        "file.inFolder(\"Tasks ☃/Projects\")",
        "file.hasTag(\"agent/job\")",
        "note[\"taskix-generated\"] == true",
        "archived != true",
    ] {
        assert!(filters.contains(&json!(filter)), "{filter}");
    }
    assert!(!source.contains("project_id"));
    assert_eq!(base["views"][0]["type"], "taskixRecentJobs");
    assert_eq!(
        base["views"][0]["pinnedColumns"],
        json!(["ACTIVE", "PENDING_REVIEW", "COMPLETED", "CANCELLED"])
    );
    assert_eq!(base["views"][0]["hideEmptyColumns"], true);
    assert!(
        !filters
            .iter()
            .any(|filter| filter.as_str().unwrap().contains("note.status"))
    );
    assert_eq!(base["views"].as_array().unwrap().len(), 5);
    for (view, status) in base["views"].as_array().unwrap()[1..].iter().zip([
        "ACTIVE",
        "PENDING_REVIEW",
        "COMPLETED",
        "CANCELLED",
    ]) {
        assert_eq!(view["limit"], 10);
        assert_eq!(view["filters"], format!("note.status == {status:?}"));
    }
    assert_eq!(
        base["views"][0]["order"],
        json!([
            "status",
            "projects",
            "formula.updated",
            "formula.review_time"
        ])
    );
    assert_eq!(
        base["formulas"]["review_time"],
        "if(note.pending_review_at, date(note.pending_review_at).format(\"YYYY-MM-DD HH:mm:ss\"), \"\")"
    );
    assert_eq!(base["views"][1]["type"], "table");
    for view in base["views"].as_array().unwrap() {
        assert_eq!(
            view["sort"][0],
            json!({"column":"updated_at","direction":"DESC"})
        );
    }
    assert_eq!(
        f.service
            .store()
            .metadata("documents")
            .await
            .unwrap()
            .unwrap()["pending-review"],
        "Recent Jobs.base"
    );
    f.service.sync().await.unwrap();
    assert_eq!(source, std::fs::read_to_string(path).unwrap());
}

#[tokio::test]
async fn recent_jobs_base_preserves_unmanaged_collision_and_recovers_publication() {
    let f = Fixture::new().await;
    let root = f.service.config().output_dir();
    let path = root.join("Recent Jobs.base");
    let service = &f.service;
    let mut paths = service
        .store()
        .metadata("documents")
        .await
        .unwrap()
        .unwrap();
    paths.as_object_mut().unwrap().remove("pending-review");
    service
        .store()
        .set_metadata("documents", &paths)
        .await
        .unwrap();
    let authored = "# My review Base\nviews: []\n";
    std::fs::write(&path, authored).unwrap();
    assert!(
        service
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("unmanaged document")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), authored);
    std::fs::write(&path, "# taskix-generated: pending-review\nviews: []\n").unwrap();
    service.sync().await.unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("PENDING_REVIEW")
    );
    f.service.sync().await.unwrap();
    assert!(path.exists());
}

pub(super) fn board_properties(f: &Fixture) -> Value {
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
    let f = Fixture::new().await;
    let root = f.service.config().output_dir();
    let text = std::fs::read_to_string(root.join("Dashboard.base")).unwrap();
    let base: Value = serde_yaml::from_str(&text).unwrap();
    assert!(!root.join("Dashboard.md").exists());
    assert!(text.starts_with("# taskix-generated: dashboard\n"));
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
    let filters: Vec<_> = filters
        .iter()
        .chain(base["views"][0]["filters"]["and"].as_array().unwrap())
        .cloned()
        .collect();
    for filter in [
        "file.inFolder(\"Tasks ☃/Projects\")",
        "file.name == \"Board\"",
        "file.ext == \"md\"",
        "file.hasTag(\"agent/project\")",
        "note.status == \"ACTIVE\"",
        "note[\"taskix-generated\"] == true",
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
async fn dashboard_base_stays_stable_while_board_records_work_activity() {
    let f = Fixture::new().await;
    let root = f.service.config().output_dir();
    let text = std::fs::read_to_string(root.join("Dashboard.base")).unwrap();
    assert!(text.contains("formula.updated"));
    let original = board_properties(&f)["updated_at"].clone();
    f.clock.fetch_add(60, Ordering::SeqCst);
    f.service.sync().await.unwrap();
    assert_eq!(
        board_properties(&f)["updated_at"],
        original,
        "sync itself is not project activity"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Dashboard.base")).unwrap(),
        text
    );
    f.task("New activity").await;
    assert_ne!(board_properties(&f)["updated_at"], original);
    assert_eq!(board_properties(&f)["updated_at"], "2026-09-05T00:01:00Z");
    assert_eq!(
        std::fs::read_to_string(root.join("Dashboard.base")).unwrap(),
        text
    );
}

#[tokio::test]
async fn dashboard_migration_protects_collisions_and_recovers_after_partial_publication() {
    let f = Fixture::new().await;
    let root = f.service.config().output_dir();
    let legacy = "---\nid: dashboard\ntaskix-generated: true\ntags: [agent/dashboard]\n---\n# Task dashboard\n";
    std::fs::write(root.join("Dashboard.md"), legacy).unwrap();
    let mut paths = f
        .service
        .store()
        .metadata("documents")
        .await
        .unwrap()
        .unwrap();
    paths["dashboard"] = json!("Dashboard.md");
    f.service
        .store()
        .set_metadata("documents", &paths)
        .await
        .unwrap();
    let service = &f.service;
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
    std::fs::write(root.join("Dashboard.md"), legacy).unwrap();
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
    f.service.sync().await.unwrap();
    assert!(!root.join("Dashboard.md").exists());
    assert!(base.is_file());
}

#[tokio::test]
async fn dashboard_review_board_and_project_boards_use_chronological_sorting() {
    let f = Fixture::new().await;
    let root = f.service.config().output_dir();
    let base: Value =
        serde_yaml::from_str(&std::fs::read_to_string(root.join("Dashboard.base")).unwrap())
            .unwrap();
    let view = &base["views"][1];
    assert_eq!(view["type"], "tasknotesKanban");
    assert_eq!(view["name"], "Pending review");
    assert_eq!(
        view["sort"][0],
        json!({"column":"pending_review_at","direction":"ASC"})
    );
    let filters = view["filters"].to_string();
    for expected in ["agent/job", "PENDING_REVIEW", "archived"] {
        assert!(filters.contains(expected), "{filters}");
    }
    let board = std::fs::read_to_string(root.join("Projects/demo/Board.md")).unwrap();
    for section in board.split("```base\n").skip(1) {
        let base: Value = serde_yaml::from_str(section.split("\n```").next().unwrap()).unwrap();
        assert_eq!(
            base["views"][0]["sort"][0],
            json!({"column":"updated_at","direction":"ASC"})
        );
    }
}
