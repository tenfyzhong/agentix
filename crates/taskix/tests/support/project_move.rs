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

#[test]
fn project_merge_cli_requires_same_remote() {
    let cli = Cli::new();
    let one = cli.dir.path().join("one");
    let two = cli.dir.path().join("two");
    std::fs::create_dir(&one).unwrap();
    std::fs::create_dir(&two).unwrap();
    let a = cli.ok(&["project", "register", "--root", one.to_str().unwrap()]);
    let b = cli.ok(&["project", "register", "--root", two.to_str().unwrap()]);
    let output = cli
        .command(&[
            "project",
            "merge",
            a["id"].as_str().unwrap(),
            "--source",
            b["id"].as_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("same nonblank Git remote"),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn project_merge_cli_preserves_jobs_and_moved_lookup_updates_root() {
    let cli = Cli::new();
    let old = cli.dir.path().join("old");
    let current = cli.dir.path().join("current");
    for root in [&old, &current] {
        std::fs::create_dir(root).unwrap();
        git(root, &["init"]);
    }
    let a = cli.ok(&[
        "project",
        "register",
        "--name",
        "demo",
        "--root",
        old.to_str().unwrap(),
    ]);
    let b = cli.ok(&[
        "project",
        "register",
        "--name",
        "demo",
        "--root",
        current.to_str().unwrap(),
    ]);
    for (project, root) in [(&a, &old), (&b, &current)] {
        git(
            root,
            &["remote", "add", "origin", "git@example.com:team/demo.git"],
        );
        cli.ok(&[
            "project",
            "mv",
            project["id"].as_str().unwrap(),
            "--root",
            root.to_str().unwrap(),
        ]);
    }
    let job = cli.ok(&[
        "job",
        "create",
        "--project",
        b["id"].as_str().unwrap(),
        "--title",
        "Retain work",
    ]);
    let args = [
        "project",
        "merge",
        b["id"].as_str().unwrap(),
        "--source",
        a["id"].as_str().unwrap(),
        "--expect-revision",
        "2",
        "--source-revision",
        "2",
        "--idempotency-key",
        "merge-cli-once",
    ];
    let merged = cli.ok(&args);
    assert_eq!(merged["id"], a["id"]);
    assert_eq!(merged["key"], "demo");
    assert_eq!(merged["root"], json!(current.canonicalize().unwrap()));
    assert_eq!(cli.ok(&args), merged);
    assert_eq!(
        cli.ok(&["job", "show", job["id"].as_str().unwrap()])["project_id"],
        a["id"]
    );
    let moved = cli.dir.path().join("moved-again");
    std::fs::rename(&current, &moved).unwrap();
    let output = cli
        .command(&["context"])
        .current_dir(&moved)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"]["project_id"],
        a["id"]
    );
    assert_eq!(
        cli.ok(&["project", "show", a["id"].as_str().unwrap()])["root"],
        json!(moved.canonicalize().unwrap())
    );
    assert_eq!(cli.ok(&["project", "list"]).as_array().unwrap().len(), 1);
}

#[test]
fn project_merge_migrates_memory_even_when_memory_service_is_disabled() {
    assert_project_memory_merge(false);
}

#[test]
fn sync_recovers_memory_after_project_merge_commits_without_memory_migration() {
    assert_project_memory_merge(true);
}

fn assert_project_memory_merge(recover: bool) {
    let cli = Cli::new();
    let mut projects = Vec::new();
    for folder in ["old", "new"] {
        let root = cli.dir.path().join(folder);
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init"]);
        let project = cli.ok(&[
            "project",
            "register",
            "--name",
            "demo",
            "--root",
            root.to_str().unwrap(),
        ]);
        git(&root, &["remote", "add", "origin", "same"]);
        cli.ok(&[
            "project",
            "mv",
            project["id"].as_str().unwrap(),
            "--root",
            root.to_str().unwrap(),
        ]);
        projects.push(project);
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let memory=runtime.block_on(async {
        let store=agentix_memory::MemoryStore::open(&cli.dir.path().join("memory.sqlite3")).await.unwrap();
        store.create(projects[1]["id"].as_str().unwrap(),serde_json::from_value(json!({"title":"Migration policy","conclusion":"Keep local backups","rationale":"User requirement","scope":"project","tags":["migration"],"kind":"user_decision","evidence":[]})).unwrap(),agentix_memory::Actor::Human).await.unwrap()
    });
    runtime.block_on(async {
        let config = agentix_task::Config::load(&cli.dir.path().join("config.toml")).unwrap();
        let store = agentix_memory::MemoryStore::open(&cli.dir.path().join("memory.sqlite3"))
            .await
            .unwrap();
        let directory = config.vault_relative_path(std::path::Path::new("Projects/demo-2/Memory"));
        agentix_memory::MemoryProjection::new(store, &config.documents.root, &directory)
            .unwrap()
            .sync(projects[1]["id"].as_str().unwrap(), "", 100)
            .await
            .unwrap();
    });
    let merged = if recover {
        let result = runtime.block_on(async {
            let config = agentix_task::Config::load(&cli.dir.path().join("config.toml")).unwrap();
            let service = agentix_task::Service::open(config).await.unwrap();
            service.execute(json!({"command":"project.merge","project":projects[0]["id"],"source":projects[1]["id"]}), agentix_task::WriteOptions::default()).await.unwrap().result
        });
        cli.ok(&["sync"]);
        result
    } else {
        cli.ok(&[
            "project",
            "merge",
            projects[0]["id"].as_str().unwrap(),
            "--source",
            projects[1]["id"].as_str().unwrap(),
        ])
    };
    runtime.block_on(async {
        let store = agentix_memory::MemoryStore::open(&cli.dir.path().join("memory.sqlite3"))
            .await
            .unwrap();
        let moved = store
            .show(merged["id"].as_str().unwrap(), &memory.id, None)
            .await
            .unwrap();
        assert_eq!(moved.content, memory.content);
        assert_eq!(
            store
                .search(merged["id"].as_str().unwrap(), "migration", 10)
                .await
                .unwrap()
                .len(),
            1
        );
    });
    let config = agentix_task::Config::load(&cli.dir.path().join("config.toml")).unwrap();
    let note = std::fs::read_to_string(
        config
            .output_dir()
            .join(format!("Projects/demo/Memory/{}.md", memory.id)),
    )
    .unwrap();
    assert!(note.contains(merged["id"].as_str().unwrap()));
    assert!(!config.output_dir().join("Projects/demo-2").exists());
    cli.ok(&["sync"]);
}
