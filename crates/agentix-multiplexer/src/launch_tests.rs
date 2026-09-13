use super::launch::persistent_launch_with_shell;
use std::io::Write;
use std::process::{Command, Stdio};

fn run(shell: &str, argv: &[String], config: &std::path::Path) -> std::process::Output {
    let launch = persistent_launch_with_shell(argv, shell);
    let mut child = Command::new(&launch[0])
        .args(&launch[1..])
        .env("XDG_CONFIG_HOME", config)
        .env("HOME", config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"echo restored-shell\nexit\n")
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn login_fish_runs_function_and_preserves_literal_arguments() {
    if Command::new("fish").arg("--version").output().is_err() {
        eprintln!("fish is unavailable; skipping real fish regression");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let fish_config = directory.path().join("fish");
    std::fs::create_dir(&fish_config).unwrap();
    std::fs::write(
        fish_config.join("config.fish"),
        r"
if status is-login
    set -gx AGENTIX_TEST_LOGIN login-loaded
    function printf
        command printf '%s\\n' function-used $AGENTIX_TEST_LOGIN $argv
        return 7
    end
end
",
    )
    .unwrap();
    let arguments = [
        "",
        "a b",
        "'",
        "\\",
        "\\'",
        "$HOME",
        "$(exit 99)",
        "`exit 99`",
        ";",
        "line\nbreak",
        "trailing\\",
    ];
    let mut argv = vec!["printf".into()];
    argv.extend(arguments.iter().map(ToString::to_string));
    let output = run("fish", &argv, directory.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = format!("function-used\nlogin-loaded\n{}\n", arguments.join("\n"));
    assert!(
        stdout.contains(&expected),
        "stdout={stdout}, stderr={:?}",
        output.stderr
    );
    assert!(stdout.contains("restored-shell"), "{stdout}");
    assert!(output.status.success());
}

#[test]
fn posix_login_launch_preserves_arguments_and_restores_shell_after_failure() {
    let directory = tempfile::tempdir().unwrap();
    let literal = "' \\\\ $HOME $(exit 99) `exit 99` ;\n";
    let argv = [
        "/bin/sh",
        "-c",
        "printf '%s' \"$1\"; exit 7",
        "agent",
        literal,
    ]
    .map(str::to_owned);
    let output = run("/bin/sh", &argv, directory.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(literal), "{stdout}");
    assert!(stdout.contains("restored-shell"), "{stdout}");
    assert!(output.status.success());
}
