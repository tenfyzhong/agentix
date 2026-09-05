use std::path::Path;
use std::process::Command;

#[test]
fn completions_skip_configuration_and_task_state_and_match_checked_in_files() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.toml");
    let invalid = directory.path().join("invalid.toml");
    let configured = directory.path().join("configured.toml");
    let database = directory.path().join("tasks.sqlite3");
    let config = agentix_task::Config {
        schema_version: 1,
        storage: agentix_task::StorageConfig {
            path: database.clone(),
        },
        documents: agentix_task::DocumentConfig {
            format: agentix_task::DocumentFormat::Markdown,
            root: directory.path().to_owned(),
            directory: "documents".into(),
        },
    };
    std::fs::write(&invalid, "not valid TOML = [").unwrap();
    std::fs::write(&configured, toml::to_string(&config).unwrap()).unwrap();

    for (shell, file, registration) in [
        ("bash", "taskcli.bash", "complete -F _taskcli"),
        ("zsh", "_taskcli", "#compdef taskcli"),
        ("fish", "taskcli.fish", "complete -c taskcli"),
    ] {
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../completions")
            .join(file);
        for config_path in [None, Some(&missing), Some(&invalid), Some(&configured)] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_taskcli"));
            if let Some(path) = config_path {
                command.arg("--config").arg(path);
            }
            let output = command
                .args(["--json", "completions", shell])
                .env("TASKCLI_CONFIG", &invalid)
                .current_dir(directory.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty());
            let script = String::from_utf8(output.stdout).unwrap();
            assert!(
                script.contains(registration),
                "{shell}: missing registration"
            );
            for command in [
                "init",
                "doctor",
                "sync",
                "project",
                "job",
                "task",
                "plan",
                "event",
                "context",
                "hook",
                "claim",
                "start",
                "done",
                "delete",
                "session-start",
                "completions",
            ] {
                assert!(script.contains(command), "{shell}: missing {command}");
            }
            for option in ["config", "lease-token", "idempotency-key", "ready", "file"] {
                assert!(script.contains(option), "{shell}: missing {option}");
            }
            assert_eq!(
                script,
                std::fs::read_to_string(&checked_in).unwrap(),
                "{shell}: run make completions to refresh the completion files"
            );
            assert!(!database.exists());
            assert!(!directory.path().join("documents").exists());
        }
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn completions_require_a_supported_shell() {
    for arguments in [vec!["completions"], vec!["completions", "invalid-shell"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_taskcli"))
            .args(&arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("<SHELL>"), "unexpected error: {error}");
        if arguments.len() == 2 {
            for shell in ["bash", "zsh", "fish"] {
                assert!(error.contains(shell), "missing {shell}: {error}");
            }
        }
    }
}

#[test]
#[cfg(unix)]
fn bash_completes_nested_commands_options_formats_and_paths() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("example config.toml"), "").unwrap();
    std::fs::write(directory.path().join("plan draft.md"), "").unwrap();
    let completion = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../completions/taskcli.bash");
    for (words, index, expected) in [
        ("taskcli ta", "1", "task"),
        ("taskcli task cl", "2", "claim"),
        ("taskcli task start --le", "3", "--lease-token"),
        ("taskcli plan create --fi", "3", "--file"),
        ("taskcli job list --ar", "3", "--archived"),
        ("taskcli init --format obs", "3", "obsidian"),
        ("taskcli completions fi", "2", "fish"),
        ("taskcli --config ex", "2", "example config.toml"),
        (
            "taskcli plan create task_ID --file pl",
            "5",
            "plan draft.md",
        ),
    ] {
        let output = Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-c",
                r#"source "$1"
read -r -a COMP_WORDS <<< "$2"
COMP_CWORD=$3
_taskcli "${COMP_WORDS[0]}" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD-1]}"
printf '%s\n' "${COMPREPLY[@]}""#,
                "completion-test",
            ])
            .arg(&completion)
            .args([words, index])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{words}: {:?}", output.stderr);
        let candidates = String::from_utf8(output.stdout).unwrap();
        assert!(
            candidates.lines().any(|candidate| candidate == expected),
            "{words}: expected {expected:?}, got {candidates:?}"
        );
    }
}
