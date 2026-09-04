use std::path::Path;

use agentix::{AgentConfig, Config, ImChannel, LogRotation, add_feishu_owner, add_telegram_owner};

#[test]
fn logging_defaults_to_info_on_stderr_only() {
    let config = Config::from_toml(
        r#"
[channel]
kind = "telegram"
[channel.telegram]
[agent]
kind = "codex"
[storage]
path = "/tmp/agentix.sqlite3"
"#,
    )
    .unwrap();

    assert_eq!(config.logging.level, "info");
    assert!(!config.logging.file.enabled);
    assert_eq!(config.logging.file.rotation, LogRotation::Daily);
    assert_eq!(config.logging.file.max_files, 7);
}

#[test]
fn parses_and_expands_rotating_file_logging_configuration() {
    let config = Config::from_toml(
        r#"
[logging]
level = "agentix=debug,agentix_codex=trace"

[logging.file]
enabled = true
path = "~/.local/state/agentix/agentix.log"
rotation = "hourly"
max_files = 12

[channel]
kind = "telegram"
[channel.telegram]
[agent]
kind = "codex"
[storage]
path = "/tmp/agentix.sqlite3"
"#,
    )
    .unwrap();

    assert_eq!(config.logging.level, "agentix=debug,agentix_codex=trace");
    assert!(config.logging.file.enabled);
    assert_eq!(config.logging.file.rotation, LogRotation::Hourly);
    assert_eq!(config.logging.file.max_files, 12);
    assert_eq!(
        config.logging.file.path,
        dirs::home_dir()
            .unwrap()
            .join(".local/state/agentix/agentix.log")
    );
}

#[test]
fn rejects_file_logging_without_retained_files() {
    let error = Config::from_toml(
        r#"
[logging.file]
enabled = true
path = "/tmp/agentix.log"
max_files = 0
[channel]
kind = "telegram"
[channel.telegram]
[agent]
kind = "codex"
[storage]
path = "/tmp/agentix.sqlite3"
"#,
    )
    .unwrap_err();

    assert!(error.to_string().contains("logging.file.max_files"));
}

#[test]
fn parses_a_codex_telegram_configuration_without_inline_secrets() {
    let config = Config::from_toml(
        r#"
[channel]
kind = "telegram"

[agent]
kind = "codex"
endpoint = "unix:///tmp/codex.sock"

[storage]
path = "/tmp/agentix.sqlite3"

[channel.telegram]
token_env = "AGENTIX_TEST_TELEGRAM_TOKEN"
owner_user_ids = [42]
"#,
    )
    .unwrap();

    assert_eq!(config.channel.kind, ImChannel::Telegram);
    #[cfg(unix)]
    assert_eq!(
        config.server.endpoint,
        format!(
            "unix://{}",
            dirs::home_dir()
                .unwrap()
                .join(".local/share/agentix/control.sock")
                .display()
        )
    );
    #[cfg(windows)]
    assert_eq!(config.server.endpoint, "tcp://127.0.0.1:32198");

    let AgentConfig::Codex {
        command,
        rmux_directory,
        ..
    } = config.agent
    else {
        panic!("expected Codex configuration");
    };
    assert_eq!(command, Path::new("codex"));
    assert_eq!(rmux_directory, dirs::home_dir().unwrap());
    assert_eq!(config.channel.telegram.unwrap().owner_user_ids, vec![42]);
}

#[test]
fn accepts_an_explicit_codex_command_for_daemon_startup() {
    let config = Config::from_toml(
        r#"
[channel]
kind = "telegram"

[agent]
kind = "codex"
command = "/opt/codex/bin/codex"

[storage]
path = "/tmp/agentix.sqlite3"

[channel.telegram]
token_env = "AGENTIX_TEST_TELEGRAM_TOKEN"
owner_user_ids = [42]
"#,
    )
    .unwrap();

    let AgentConfig::Codex { command, .. } = config.agent else {
        panic!("expected Codex configuration");
    };
    assert_eq!(command, Path::new("/opt/codex/bin/codex"));
}

#[test]
fn expands_home_in_codex_configuration_paths() {
    let home = dirs::home_dir().expect("the test user should have a home directory");
    let config = Config::from_toml(
        r#"
[channel]
kind = "telegram"

[agent]
kind = "codex"
command = "~/.local/bin/codex"
endpoint = "unix://~/.codex/custom.sock"
rmux_directory = "~/workspace"

[storage]
path = "~/.local/share/agentix/state.sqlite3"

[channel.telegram]
token_env = "AGENTIX_TEST_TELEGRAM_TOKEN"
owner_user_ids = [42]
"#,
    )
    .unwrap();

    let AgentConfig::Codex {
        command,
        endpoint,
        rmux_directory,
    } = config.agent
    else {
        panic!("expected Codex configuration");
    };
    assert_eq!(command, home.join(".local/bin/codex"));
    assert_eq!(
        endpoint,
        format!("unix://{}", home.join(".codex/custom.sock").display())
    );
    assert_eq!(rmux_directory, home.join("workspace"));
    assert_eq!(
        config.storage.path,
        home.join(".local/share/agentix/state.sqlite3")
    );
}

#[test]
fn accepts_the_legacy_multiplexer_directory_alias() {
    let config = Config::from_toml(
        r#"
[channel]
kind = "telegram"

[agent]
kind = "codex"
multiplexer_directory = "~/workspace"

[storage]
path = "~/.local/state/agentix/state.db"

[channel.telegram]
owner_user_ids = [42]
"#,
    )
    .unwrap();

    let AgentConfig::Codex { rmux_directory, .. } = config.agent else {
        panic!("expected Codex configuration");
    };
    assert_eq!(rmux_directory, dirs::home_dir().unwrap().join("workspace"));
}

#[test]
fn expands_home_in_pi_and_oh_my_pi_configuration_paths() {
    let home = dirs::home_dir().expect("the test user should have a home directory");

    for (kind, command, session_dir) in [
        ("pi", "~/.local/bin/pi", "~/.pi/agent/sessions"),
        ("oh-my-pi", "~/.local/bin/omp", "~/.omp/agent/sessions"),
    ] {
        let config = Config::from_toml(&format!(
            r#"
[channel]
kind = "telegram"

[agent]
kind = "{kind}"
command = "{command}"
session_dir = "{session_dir}"

[storage]
path = "~/agentix.sqlite3"

[channel.telegram]
owner_user_ids = [42]
"#
        ))
        .unwrap();

        let (actual_command, actual_session_dir) = match config.agent {
            AgentConfig::Pi {
                command,
                session_dir,
            }
            | AgentConfig::OhMyPi {
                command,
                session_dir,
            } => (command, session_dir),
            AgentConfig::Codex { .. } => panic!("expected a Pi-compatible configuration"),
        };
        assert_eq!(actual_command, home.join(command.trim_start_matches("~/")));
        assert_eq!(
            actual_session_dir,
            home.join(session_dir.trim_start_matches("~/"))
        );
        assert_eq!(config.storage.path, home.join("agentix.sqlite3"));
    }
}

#[test]
fn expands_a_standalone_home_marker() {
    let home = dirs::home_dir().expect("the test user should have a home directory");
    let config = Config::from_toml(
        r#"
[channel]
kind = "telegram"

[agent]
kind = "codex"

[storage]
path = "~"

[channel.telegram]
owner_user_ids = [42]
"#,
    )
    .unwrap();

    assert_eq!(config.storage.path, home);
}

#[test]
fn leaves_named_user_tildes_unchanged() {
    let config = Config::from_toml(
        r#"
[channel]
kind = "telegram"

[agent]
kind = "codex"
command = "~someone/bin/codex"
endpoint = "unix://~someone/.codex/socket"

[storage]
path = "~someone/agentix.sqlite3"

[channel.telegram]
owner_user_ids = [42]
"#,
    )
    .unwrap();

    let AgentConfig::Codex {
        command, endpoint, ..
    } = config.agent
    else {
        panic!("expected Codex configuration");
    };
    assert_eq!(command, Path::new("~someone/bin/codex"));
    assert_eq!(endpoint, "unix://~someone/.codex/socket");
    assert_eq!(config.storage.path, Path::new("~someone/agentix.sqlite3"));
}

#[test]
fn requires_an_explicit_channel_with_matching_configuration() {
    let no_selection = Config::from_toml(
        r#"
[agent]
kind = "pi"
session_dir = "/tmp/pi-sessions"
[storage]
path = "/tmp/agentix.sqlite3"

[channel.telegram]
owner_user_ids = [42]
"#,
    )
    .unwrap_err();
    assert!(format!("{no_selection:#}").contains("missing field `kind`"));

    let missing_selected_config = Config::from_toml(
        r#"
[channel]
kind = "telegram"

[agent]
kind = "pi"
session_dir = "/tmp/pi-sessions"
[storage]
path = "/tmp/agentix.sqlite3"
"#,
    )
    .unwrap_err();
    assert!(
        missing_selected_config
            .to_string()
            .contains("selected telegram channel requires [channel.telegram]")
    );

    let no_owners = Config::from_toml(
        r#"
[channel]
kind = "feishu"

[agent]
kind = "oh-my-pi"
session_dir = "/tmp/omp-sessions"
[storage]
path = "/tmp/agentix.sqlite3"
[channel.feishu]
app_id_env = "APP_ID"
app_secret_env = "APP_SECRET"
"#,
    )
    .unwrap();
    assert!(
        no_owners
            .channel
            .feishu
            .expect("Feishu configuration should be present")
            .owner_open_ids
            .is_empty()
    );

    let telegram_without_owners = Config::from_toml(
        r#"
[channel]
kind = "telegram"

[agent]
kind = "codex"
[storage]
path = "/tmp/agentix.sqlite3"
[channel.telegram]
token_env = "TELEGRAM_TOKEN"
"#,
    )
    .unwrap();
    assert!(
        telegram_without_owners
            .channel
            .telegram
            .unwrap()
            .owner_user_ids
            .is_empty()
    );
}

#[test]
fn claimed_feishu_owner_is_written_without_losing_config_formatting() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        r#"# Keep this comment.
[channel]
kind = "feishu"

[channel.feishu]
app_id_env = "APP_ID"
app_secret_env = "APP_SECRET"
owner_open_ids = [] # Claimed owners.

[agent]
kind = "codex"

[storage]
path = "~/agentix.sqlite3"
"#,
    )
    .unwrap();

    add_feishu_owner(&path, "ou_claimed").unwrap();
    add_feishu_owner(&path, "ou_claimed").unwrap();

    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("# Keep this comment."));
    assert!(updated.contains("owner_open_ids = [\"ou_claimed\"] # Claimed owners."));
    assert_eq!(updated.matches("ou_claimed").count(), 1);
    assert_eq!(
        Config::load(&path)
            .unwrap()
            .channel
            .feishu
            .unwrap()
            .owner_open_ids,
        vec!["ou_claimed"]
    );

    let path_without_owner_field = directory.path().join("config-without-owner.toml");
    std::fs::write(
        &path_without_owner_field,
        r#"[channel]
kind = "feishu"
[channel.feishu]
app_id_env = "APP_ID"
app_secret_env = "APP_SECRET"
[agent]
kind = "codex"
[storage]
path = "~/agentix.sqlite3"
"#,
    )
    .unwrap();
    add_feishu_owner(&path_without_owner_field, "ou_claimed").unwrap();
    assert_eq!(
        Config::load(&path_without_owner_field)
            .unwrap()
            .channel
            .feishu
            .unwrap()
            .owner_open_ids,
        vec!["ou_claimed"]
    );
}

#[test]
fn claimed_telegram_owner_is_written_without_losing_config_formatting() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        r#"# Keep this comment.
[channel]
kind = "telegram"

[channel.telegram]
token_env = "TELEGRAM_TOKEN"
owner_user_ids = [] # Claimed owners.

[agent]
kind = "codex"

[storage]
path = "~/agentix.sqlite3"
"#,
    )
    .unwrap();

    add_telegram_owner(&path, 42).unwrap();
    add_telegram_owner(&path, 42).unwrap();

    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("# Keep this comment."));
    assert!(updated.contains("owner_user_ids = [42] # Claimed owners."));
    assert_eq!(
        Config::load(&path)
            .unwrap()
            .channel
            .telegram
            .unwrap()
            .owner_user_ids,
        vec![42]
    );
}

#[test]
fn accepts_an_explicit_agentix_control_endpoint() {
    let config = Config::from_toml(
        r#"
[server]
endpoint = "tcp://127.0.0.1:32198"

[channel]
kind = "telegram"

[agent]
kind = "codex"

[storage]
path = "/tmp/agentix.sqlite3"

[channel.telegram]
owner_user_ids = [42]
"#,
    )
    .unwrap();

    assert_eq!(config.server.endpoint, "tcp://127.0.0.1:32198");
}

#[test]
fn resolves_credentials_only_from_named_environment_entries() {
    let config = Config::from_toml(
        r#"
[channel]
kind = "feishu"

[agent]
kind = "codex"
endpoint = "unix:///tmp/codex.sock"
[storage]
path = "/tmp/agentix.sqlite3"
[channel.feishu]
app_id_env = "FEISHU_ID"
app_secret_env = "FEISHU_SECRET"
owner_open_ids = ["ou_owner"]
"#,
    )
    .unwrap();
    let credentials = config
        .feishu_credentials_with(|name| match name {
            "FEISHU_ID" => Some("cli_a".into()),
            "FEISHU_SECRET" => Some("secret".into()),
            _ => None,
        })
        .unwrap()
        .unwrap();

    assert_eq!(credentials.0, "cli_a");
    assert_eq!(credentials.1, "secret");
}

#[test]
fn only_the_explicitly_selected_channel_is_active() {
    let config = Config::from_toml(
        r#"
[channel]
kind = "feishu"

[agent]
kind = "codex"

[storage]
path = "/tmp/agentix.sqlite3"

[channel.telegram]
token_env = "UNSET_TELEGRAM_TOKEN"
owner_user_ids = []

[channel.feishu]
app_id_env = "FEISHU_ID"
app_secret_env = "FEISHU_SECRET"
owner_open_ids = ["ou_owner"]
"#,
    )
    .unwrap();

    assert_eq!(config.channel.kind, ImChannel::Feishu);
    assert_eq!(
        config
            .telegram_token_with(|_| panic!("inactive Telegram credentials were read"))
            .unwrap(),
        None
    );
    assert_eq!(
        config
            .feishu_credentials_with(|name| Some(format!("value-for-{name}")))
            .unwrap(),
        Some((
            "value-for-FEISHU_ID".into(),
            "value-for-FEISHU_SECRET".into()
        ))
    );
}
