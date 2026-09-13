/// Start an agent through the user's login shell, then restore an interactive
/// prompt even after failure. The outer interactive shell preserves job control.
#[cfg(unix)]
#[must_use]
pub fn persistent_launch_argv(argv: &[String]) -> Vec<String> {
    persistent_launch_with_shell(argv, &login_shell())
}

/// Initial command for a Unix pane; other platforms retain their native default.
#[must_use]
pub fn interactive_login_shell_argv() -> Option<Vec<String>> {
    #[cfg(unix)]
    {
        Some(vec![login_shell(), "-il".into()])
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn login_shell() -> String {
    use nix::unistd::{Uid, User};
    std::env::var("AGENTIX_LOGIN_SHELL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            User::from_uid(Uid::effective())
                .ok()
                .flatten()
                .and_then(|user| user.shell.into_os_string().into_string().ok())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            std::env::var("SHELL")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "/bin/sh".into())
}

#[cfg(unix)]
pub(super) fn persistent_launch_with_shell(argv: &[String], shell: &str) -> Vec<String> {
    let fish = std::path::Path::new(shell)
        .file_name()
        .is_some_and(|name| name == "fish");
    let command = argv
        .iter()
        .map(|argument| {
            // Fish interprets backslashes inside single quotes; POSIX shells do not.
            let escaped = if fish {
                argument.replace('\\', "\\\\")
            } else {
                argument.clone()
            };
            format!("'{}'", escaped.replace('\'', "'\\''"))
        })
        .collect::<Vec<_>>()
        .join(" ");
    // Non-interactive fish otherwise keeps children in its own process group,
    // hiding the agent from pane foreground detection and terminal signals.
    let command = if fish {
        format!("status job-control full; {command}")
    } else {
        command
    };
    vec![
        "/bin/sh".into(),
        "-i".into(),
        "-c".into(),
        // Do not exec the agent: shell functions must participate in lookup.
        r#""$1" -lc "$2"; exec "$1" -i"#.into(),
        "agentix".into(),
        shell.into(),
        command,
    ]
}

#[cfg(not(unix))]
#[must_use]
pub fn persistent_launch_argv(argv: &[String]) -> Vec<String> {
    argv.to_vec()
}
