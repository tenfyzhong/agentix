use super::*;

fn git(root: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "core.hooksPath="])
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn project_move_rebinds_renamed_git_repository_and_refreshes_origin() {
    let cli = Cli::new();
    let old = cli.dir.path().join("old");
    let new = cli.dir.path().join("new");
    std::fs::create_dir(&old).unwrap();
    git(&old, &["init"]);
    git(
        &old,
        &["remote", "add", "origin", "https://example.com/old.git"],
    );
    let project = cli.ok(&["project", "register", "--root", old.to_str().unwrap()]);
    let id = project["id"].as_str().unwrap();
    let job = cli.ok(&["job", "create", "--project", id, "--title", "Existing work"]);
    std::fs::rename(&old, &new).unwrap();
    git(
        &new,
        &["remote", "set-url", "origin", "https://example.com/new.git"],
    );
    let moved = cli.ok(&[
        "project",
        "mv",
        id,
        "--root",
        new.to_str().unwrap(),
        "--expect-revision",
        "1",
    ]);
    assert_eq!(moved["id"], project["id"]);
    assert_eq!(moved["root"], json!(new.canonicalize().unwrap()));
    assert_eq!(moved["remote"], "https://example.com/new.git");
    assert_eq!(moved["name"], project["name"]);
    let output = cli
        .command(&["context"])
        .current_dir(&new)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"]["project_id"],
        project["id"]
    );
    assert_eq!(cli.ok(&["project", "list"]).as_array().unwrap().len(), 1);
    assert_eq!(cli.ok(&["job", "show", job["id"].as_str().unwrap()]), job);
    git(&new, &["remote", "remove", "origin"]);
    assert!(cli.ok(&["project", "mv", id, "--root", new.to_str().unwrap()])["remote"].is_null());
}

#[test]
fn project_move_uses_current_directory_and_explicit_remote_for_non_git_projects() {
    let cli = Cli::new();
    let old = cli.dir.path().join("old");
    let new = cli.dir.path().join("new");
    std::fs::create_dir(&old).unwrap();
    let project = cli.ok(&[
        "project",
        "register",
        "--name",
        "Stable",
        "--root",
        old.to_str().unwrap(),
    ]);
    std::fs::rename(old, &new).unwrap();
    let output = cli
        .command(&[
            "project",
            "mv",
            "Stable",
            "--remote",
            "https://example.com/explicit.git",
        ])
        .current_dir(&new)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let moved = serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"].clone();
    assert_eq!(moved["id"], project["id"]);
    assert_eq!(moved["root"], json!(new.canonicalize().unwrap()));
    assert_eq!(moved["remote"], "https://example.com/explicit.git");
    let output = cli.command(&["context"]).current_dir(new).output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"]["project_id"],
        project["id"]
    );
    assert_eq!(cli.ok(&["project", "list"]).as_array().unwrap().len(), 1);
}

#[test]
fn project_move_from_worktree_uses_common_repository_identity() {
    let cli = Cli::new();
    let repo = cli.dir.path().join("repository");
    let worktree = cli.dir.path().join("worktree");
    let old = cli.dir.path().join("old");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir(&old).unwrap();
    git(&repo, &["init"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--signoff",
            "--allow-empty",
            "-m",
            "Initial",
        ],
    );
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://example.com/repository.git",
        ],
    );
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            worktree.to_str().unwrap(),
        ],
    );
    let project = cli.ok(&["project", "register", "--root", old.to_str().unwrap()]);
    let moved = cli.ok(&[
        "project",
        "mv",
        project["id"].as_str().unwrap(),
        "--root",
        worktree.to_str().unwrap(),
    ]);
    assert_eq!(moved["root"], json!(repo.canonicalize().unwrap()));
    assert_eq!(moved["remote"], "https://example.com/repository.git");
    for path in [&repo, &worktree] {
        let output = cli
            .command(&["context"])
            .current_dir(path)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"]["project_id"],
            project["id"]
        );
    }
}

#[test]
fn project_mv_rejects_move_subcommand() {
    let cli = Cli::new();
    let output = cli.run(&["project", "move", "unused"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
}
