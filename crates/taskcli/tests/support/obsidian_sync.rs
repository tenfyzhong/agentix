use super::*;

#[test]
fn obsidian_snapshot_identifies_unplanned_renamed_and_archived_notes_without_credentials() {
    let cli = Cli::new("obsidian");
    let job = cli.job("Status bridge");
    let task = cli.task(&job, "Unplanned");
    let before = cli.ok(&["obsidian", "snapshot"]);
    assert_eq!(before["documents"]["format"], "obsidian");
    let find = |snapshot: &Value, id: &str| {
        snapshot["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .unwrap()
            .clone()
    };
    let note = find(&before, &task);
    assert_eq!(note["kind"], "task");
    assert_eq!(note["status"], "TODO");
    assert!(
        note["path"]
            .as_str()
            .unwrap()
            .starts_with("Tasks \u{2603}/Projects/Demo/Tasks/")
    );
    assert!(
        cli.dir
            .path()
            .join("vault")
            .join(note["path"].as_str().unwrap())
            .is_file()
    );
    let claim = cli.claim(&task, "bridge");
    let snapshot = cli.ok(&["obsidian", "snapshot"]);
    assert!(
        !snapshot
            .to_string()
            .contains(claim["lease"]["token"].as_str().unwrap())
    );
    cli.owned(&["task", "cancel", &task], &claim);
    cli.ok(&["task", "update", &task, "--name", "Renamed"]);
    cli.ok(&["job", "cancel", &job]);
    cli.ok(&["job", "archive", &job]);
    let after = cli.ok(&["obsidian", "snapshot"]);
    assert!(
        find(&after, &task)["path"]
            .as_str()
            .unwrap()
            .ends_with("-Renamed.md")
    );
    assert!(
        find(&after, &job)["path"]
            .as_str()
            .unwrap()
            .contains("/Jobs/Archived/")
    );
    assert_eq!(find(&after, &task)["properties"]["status"], "CANCELLED");
    assert!(find(&after, &task)["revision"].as_i64().unwrap() > note["revision"].as_i64().unwrap());
}
