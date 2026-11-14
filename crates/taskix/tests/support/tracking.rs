use super::*;

fn exclude(cli: &Cli, path: &std::path::Path) {
    let file = cli.dir.path().join("config.toml");
    let mut value: toml::Value = toml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    value["tracking"] = toml::Value::try_from(json!({"exclude_paths":[path]})).unwrap();
    std::fs::write(file, toml::to_string(&value).unwrap()).unwrap();
}

#[test]
fn excluded_non_git_descendants_skip_automatic_context_and_capture() {
    let cli = Cli::new();
    let excluded = cli.dir.path().join("private");
    let nested = excluded.join("a/b/c");
    std::fs::create_dir_all(&nested).unwrap();
    exclude(&cli, &excluded);
    for command in [vec!["context"], vec!["routing", "snapshot"]] {
        let output = cli.command(&command).current_dir(&nested).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["result"]["tracking"]["enabled"], false);
        assert!(value["result"]["project_id"].is_null());
    }
    let output = cli
        .command(&[
            "hook",
            "record",
            "--session",
            "excluded",
            "--file",
            "/missing/capture.json",
        ])
        .current_dir(&nested)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "excluded capture must skip reading its file"
    );
    assert_eq!(cli.ok(&["project", "list"]), json!([]));
    let manual = cli.ok(&["project", "register", "--root", nested.to_str().unwrap()]);
    assert!(manual["id"].is_string());
}

#[test]
fn project_check_is_read_only_without_storage_or_vault() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[tracking]\nexclude_paths = [{}]\n",
            serde_json::to_string(dir.path().to_str().unwrap()).unwrap()
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_taskix"))
        .args(["--json", "--config"])
        .arg(&config)
        .args(["project", "check", "--path"])
        .arg(dir.path().join("future/a"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["tracking"]["enabled"], false);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn path_rules_match_components_and_future_paths() {
    let cli = Cli::new();
    let base = cli.dir.path().join("private");
    exclude(&cli, &base);
    for (suffix, enabled) in [
        ("private", false),
        ("private/a/b", false),
        ("private-other", true),
    ] {
        let value = cli.ok(&[
            "project",
            "check",
            "--path",
            cli.dir.path().join(suffix).to_str().unwrap(),
        ]);
        assert_eq!(value["tracking"]["enabled"], enabled, "{suffix}");
    }
}

#[cfg(unix)]
#[test]
fn symlinks_and_worktrees_preserve_exclusion_scope() {
    let cli = Cli::new();
    let repo = cli.dir.path().join("repo");
    let worktree = cli.dir.path().join("worktree");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "test",
        ],
        vec!["worktree", "add", "-b", "test", worktree.to_str().unwrap()],
    ] {
        assert!(
            Command::new("git")
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null"
                ])
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let alias = cli.dir.path().join("alias");
    std::os::unix::fs::symlink(&worktree, &alias).unwrap();
    exclude(&cli, &worktree);
    for (path, enabled) in [(&repo, true), (&worktree, false), (&alias, false)] {
        assert_eq!(
            cli.ok(&["project", "check", "--path", path.to_str().unwrap()])["tracking"]["enabled"],
            enabled
        );
    }
    // Replacing the rule with the main root excludes the external worktree too.
    let config = cli.dir.path().join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        config,
        text.replace(worktree.to_str().unwrap(), repo.to_str().unwrap()),
    )
    .unwrap();
    assert_eq!(
        cli.ok(&["project", "check", "--path", worktree.to_str().unwrap()])["tracking"]["enabled"],
        false
    );
}
