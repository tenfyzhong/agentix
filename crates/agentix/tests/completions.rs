use std::path::Path;
use std::process::Command;

#[test]
fn completions_work_without_configuration_and_match_checked_in_files() {
    let directory = tempfile::tempdir().unwrap();
    let missing_config = directory.path().join("missing.toml");
    for (shell, file, registration) in [
        ("bash", "agentix.bash", "complete -F _agentix"),
        ("zsh", "_agentix", "#compdef agentix"),
        ("fish", "agentix.fish", "complete -c agentix"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
            .arg("--config")
            .arg(&missing_config)
            .args(["completions", shell])
            .env("RUST_LOG", "invalid[filter")
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
            "serve",
            "doctor",
            "client",
            "claim",
            "sessions",
            "call",
            "completions",
        ] {
            assert!(script.contains(command), "{shell}: missing {command}");
        }
        for option in ["config", "ttl-minutes", "cursor", "limit", "params"] {
            assert!(script.contains(option), "{shell}: missing {option}");
        }
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../completions")
            .join(file);
        assert_eq!(
            script,
            std::fs::read_to_string(checked_in).unwrap(),
            "{shell}: run make completions to refresh the completion files"
        );
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn completions_require_a_supported_shell() {
    for arguments in [vec!["completions"], vec!["completions", "invalid-shell"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
            .args(&arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("<SHELL>"));
        if arguments.len() == 2 {
            assert!(error.contains("bash"));
            assert!(error.contains("zsh"));
            assert!(error.contains("fish"));
        }
    }
}

#[test]
#[cfg(unix)]
fn bash_completes_nested_commands_options_and_config_paths() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("example config.toml"), "").unwrap();
    let completion = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../completions/agentix.bash");
    for (words, index, expected) in [
        ("agentix cl", "1", "client"),
        ("agentix client se", "2", "sessions"),
        ("agentix client claim --tt", "3", "--ttl-minutes"),
        ("agentix client sessions --li", "3", "--limit"),
        ("agentix completions fi", "2", "fish"),
        ("agentix --config ex", "2", "example config.toml"),
    ] {
        let output = Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-c",
                r#"source "$1"
read -r -a COMP_WORDS <<< "$2"
COMP_CWORD=$3
_agentix "${COMP_WORDS[0]}" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD-1]}"
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
