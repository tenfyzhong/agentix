use super::*;
use agentix_domain::MultiplexerMutation;
use agentix_multiplexer::WorkspaceManager;
use std::sync::Arc;

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["-S"])
            .arg(&self.0)
            .arg("kill-server")
            .output();
    }
}
#[test]
fn parses_panes_without_splitting_names_or_directories_on_spaces() {
    let panes = parse_inventory(
        "$1|work space|@2|3|code ' test|%4|0|1|fish|/a path/'quote|42|/dev/pts/7\n",
    )
    .unwrap();
    assert_eq!(panes[0].pane_id, "%4");
    assert_eq!(panes[0].cwd, "/a path/'quote");
    assert_eq!(panes[0].window_name, "code ' test");
    assert_eq!(panes[0].multiplexer, MultiplexerKind::Tmux);
    assert!(parse_inventory("$1|broken\n").is_err());
}
#[test]
fn matches_descendant_processes_to_the_original_pane() {
    let panes = parse_inventory("$1|work|@2|0|code|%4|0|1|fish|/tmp|42|/dev/pts/7\n").unwrap();
    let locations = descendant_locations(&panes, "42 1\n43 42\n44 43\n55 1\n");
    assert_eq!(locations[&44].pane_id, "%4");
    assert!(!locations.contains_key(&55));
}
#[tokio::test]
async fn missing_binary_is_reported_without_falling_back() {
    let driver = TmuxDriver::with_command(PathBuf::from("/missing/agentix-test-tmux"), None);
    assert!(driver.inventory(false).await.is_err());
}
#[cfg(unix)]
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_workspace_creation_and_launch_preserve_arguments() {
    if std::env::var_os("AGENTIX_TEST_TMUX").is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("tmux.sock");
    let driver = Arc::new(TmuxDriver::with_command(
        "tmux".into(),
        Some(socket.clone()),
    ));
    let mut manager = WorkspaceManager::new(vec!["sleep".into(), "30".into()], root.path());
    manager.set_driver(driver.clone());
    let _cleanup = Cleanup(socket);
    assert!(driver.inventory(false).await.unwrap().unwrap().is_empty());
    assert!(!driver.probe().await.unwrap());
    let mutation = MultiplexerMutation {
        target: MultiplexerTarget::NewSession {
            name: "test".into(),
            cwd: root.path().display().to_string(),
        },
        launch_agent: false,
    };
    let prepared = manager.prepare(mutation).await.unwrap();
    let created = match manager.execute(&prepared).await {
        Ok(value) => value,
        Err(error) => panic!(
            "{error}: {:?}",
            driver
                .run(&strings(&["list-panes", "-a", "-F", FORMAT]))
                .await
        ),
    };
    assert!(driver.probe().await.unwrap());
    assert!(manager.pane_exists(&created.location).await.unwrap());
    let pane = created.location.pane_id;
    let session_id = driver.inventory(false).await.unwrap().unwrap()[0]
        .session_id
        .clone();
    for target in [
        MultiplexerTarget::NewWindow {
            session_id,
            name: "second".into(),
            cwd: root.path().display().to_string(),
        },
        MultiplexerTarget::SplitPane {
            pane_id: pane.clone(),
            direction: PaneSplitDirection::Horizontal,
            cwd: root.path().display().to_string(),
        },
        MultiplexerTarget::SplitPane {
            pane_id: pane.clone(),
            direction: PaneSplitDirection::Vertical,
            cwd: root.path().display().to_string(),
        },
        MultiplexerTarget::ExistingPane {
            pane_id: pane.clone(),
        },
    ] {
        let prepared = manager
            .prepare(MultiplexerMutation {
                target,
                launch_agent: true,
            })
            .await
            .unwrap();
        let result = manager.execute(&prepared).await.unwrap();
        assert!(manager.pane_exists(&result.location).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        manager
            .prepare(MultiplexerMutation {
                target: MultiplexerTarget::ExistingPane { pane_id: pane },
                launch_agent: true
            })
            .await
            .is_err()
    );
    assert_eq!(driver.inventory(false).await.unwrap().unwrap().len(), 4);
    let arguments = [
        "space value",
        "quote'\\\"",
        "中文",
        "$(touch /tmp/agentix-not-executed)",
        "a;",
    ];
    let mut argv = vec!["/usr/bin/printf".to_owned(), "%s\\n".to_owned()];
    argv.extend(arguments.iter().map(|value| (*value).to_owned()));
    manager.set_argv(argv);
    let prepared = manager
        .prepare(MultiplexerMutation {
            target: MultiplexerTarget::NewSession {
                name: "arguments".into(),
                cwd: root.path().display().to_string(),
            },
            launch_agent: true,
        })
        .await
        .unwrap();
    let result = manager.execute(&prepared).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let output = driver
        .output(&[
            "capture-pane".into(),
            "-p".into(),
            "-S".into(),
            "-".into(),
            "-t".into(),
            result.location.pane_id,
        ])
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with(&arguments.join("\n")),
        "{:?}",
        output.stdout
    );
}

#[cfg(unix)]
#[tokio::test]
async fn native_agent_exit_restores_usable_shell() {
    if std::env::var_os("AGENTIX_TEST_TMUX").is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("tmux.sock");
    let driver = TmuxDriver::with_command("tmux".into(), Some(socket.clone()));
    let _cleanup = Cleanup(socket);
    let pane = driver
        .run(&strings(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            "exit-test",
        ]))
        .await
        .unwrap();
    let pane = pane.trim();
    for status in ["0", "7", "interrupt"] {
        let argv = if status == "interrupt" {
            strings(&["/bin/sleep", "30"])
        } else {
            strings(&["/bin/sh", "-c", &format!("exit {status}")])
        };
        driver
            .launch(pane, &root.path().display().to_string(), &argv)
            .await
            .unwrap();
        if status == "interrupt" {
            wait_for_pane_command(&driver, pane, "sleep").await;
            driver
                .run(&strings(&["send-keys", "-t", pane, "C-c"]))
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = driver
                    .run(&strings(&[
                        "display-message",
                        "-p",
                        "-t",
                        pane,
                        "#{pane_dead}|#{pane_current_command}",
                    ]))
                    .await
                    .unwrap();
                if state
                    .trim()
                    .strip_prefix("0|")
                    .is_some_and(is_shell_command)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("agent exit must leave a live interactive shell");
        let marker = format!("result-{status}");
        driver
            .run(&strings(&[
                "send-keys",
                "-t",
                pane,
                "-l",
                &format!("echo usable > {marker}"),
            ]))
            .await
            .unwrap();
        driver
            .run(&strings(&["send-keys", "-t", pane, "Enter"]))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if std::fs::read_to_string(root.path().join(&marker))
                    .ok()
                    .as_deref()
                    == Some("usable\n")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("shell must accept input in the requested directory");
    }
    assert_ctrl_d_closes_pane(&driver, pane).await;
}

#[cfg(unix)]
async fn wait_for_pane_command(driver: &TmuxDriver, pane: &str, command: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = driver
                .run(&strings(&[
                    "display-message",
                    "-p",
                    "-t",
                    pane,
                    "#{pane_current_command}",
                ]))
                .await
                .unwrap();
            if current.trim() == command {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("agent must be visible as the foreground command");
}

#[cfg(unix)]
async fn assert_ctrl_d_closes_pane(driver: &TmuxDriver, pane: &str) {
    let other = driver
        .run(&strings(&["new-window", "-d", "-P", "-F", "#{pane_id}"]))
        .await
        .unwrap();
    driver
        .run(&strings(&["send-keys", "-t", pane, "C-d"]))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let panes = driver.inventory(false).await.unwrap().unwrap();
            assert!(panes.iter().any(|p| p.pane_id == other.trim()));
            if panes.iter().all(|p| p.pane_id != pane) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("Ctrl-D must remove the pane, not leave a dead pane");
}

#[tokio::test]
async fn probe_requires_a_successful_command_response() {
    let driver = TmuxDriver::with_command("/missing/agentix-test-tmux".into(), None);
    assert!(driver.probe().await.is_err());
}

#[tokio::test]
async fn probe_does_not_start_a_missing_server() {
    if std::env::var_os("AGENTIX_TEST_TMUX").is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("absent.sock");
    let driver = TmuxDriver::with_command("tmux".into(), Some(socket.clone()));
    assert!(!driver.probe().await.unwrap());
    assert!(!socket.exists());
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires an installed rmux daemon"]
async fn probe_accepts_rmux_compatibility_interface() {
    struct Server(PathBuf);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::process::Command::new("rmux")
                .arg("-S")
                .arg(&self.0)
                .arg("kill-server")
                .output();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("compat.sock");
    let driver = TmuxDriver::with_command("rmux".into(), Some(socket.clone()));
    let _cleanup = Server(socket);
    assert!(!driver.probe().await.unwrap());
    driver
        .run(&strings(&["new-session", "-d", "-s", "probe-test"]))
        .await
        .unwrap();
    assert!(driver.probe().await.unwrap());
}
