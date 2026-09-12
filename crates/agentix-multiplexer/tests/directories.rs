use agentix_multiplexer::WorkspaceManager;

#[tokio::test]
async fn directories_preserve_paths_and_reject_invalid_targets() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().canonicalize().unwrap();
    let manager = WorkspaceManager::new(Vec::new(), &home);
    let name = "work tree 中文 $HOME ; $(no)";
    std::fs::create_dir(home.join(name)).unwrap();
    assert_eq!(
        manager
            .resolve_directory("~", &home.to_string_lossy())
            .await
            .unwrap(),
        home.to_string_lossy()
    );
    assert_eq!(
        manager
            .resolve_directory(name, &home.to_string_lossy())
            .await
            .unwrap(),
        home.join(name).to_string_lossy()
    );
    assert_eq!(
        manager
            .resolve_directory(&format!("~/{name}"), "/")
            .await
            .unwrap(),
        home.join(name).to_string_lossy()
    );
    std::fs::write(home.join("file"), "data").unwrap();
    for input in ["file", "missing", "", "\0"] {
        assert!(
            manager
                .resolve_directory(input, &home.to_string_lossy())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn directories_are_sorted_paged_and_include_hidden_on_request() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().canonicalize().unwrap();
    let manager = WorkspaceManager::new(Vec::new(), &home);
    for name in ["g", "a", "d", "b", "f", "e", "c", ".git"] {
        std::fs::create_dir(home.join(name)).unwrap();
    }
    std::fs::write(home.join("file"), "data").unwrap();
    let page = manager
        .list_directories(&home.to_string_lossy(), 0, false)
        .await
        .unwrap();
    assert_eq!(
        page.entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c", "d", "e", "f"]
    );
    assert_eq!(page.pages, 2);
    let last = manager
        .list_directories(&home.to_string_lossy(), 99, false)
        .await
        .unwrap();
    assert_eq!(last.page, 1);
    assert_eq!(last.entries[0].name, "g");
    let hidden = manager
        .list_directories(&home.to_string_lossy(), 0, true)
        .await
        .unwrap();
    assert_eq!(hidden.entries[0].name, ".git");
}

#[cfg(unix)]
#[tokio::test]
async fn directory_links_resolve_without_changing_worktree_identity() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().canonicalize().unwrap();
    let worktree = home.join(".git/wtm/feature");
    std::fs::create_dir_all(&worktree).unwrap();
    std::os::unix::fs::symlink(&worktree, home.join("link")).unwrap();
    let manager = WorkspaceManager::new(Vec::new(), &home);
    assert_eq!(
        manager
            .resolve_directory("link", &home.to_string_lossy())
            .await
            .unwrap(),
        worktree.to_string_lossy()
    );
    let page = manager
        .list_directories(&home.to_string_lossy(), 0, false)
        .await
        .unwrap();
    assert_eq!(page.entries[0].name, "link");
}
