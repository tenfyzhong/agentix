use agentix::Config;

const BASE: &str = "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[storage]\npath='/tmp/state'\n[agent]\nkind='codex'\n";

#[test]
fn accepts_global_multiplexer_configuration() {
    let config = Config::from_toml(&format!(
        "{BASE}\n[multiplexer]\nkind='tmux'\nworking_dir='~/work'\n"
    ));
    let config = config.unwrap();
    assert_eq!(config.multiplexer.kind, agentix_core::MultiplexerKind::Tmux);
    assert_eq!(
        config.multiplexer.working_dir,
        dirs::home_dir().unwrap().join("work")
    );
}

#[test]
fn rejects_removed_agent_directory_fields() {
    for field in ["rmux_directory", "multiplexer_directory"] {
        assert!(
            Config::from_toml(&format!("{BASE}{field}='~'\n")).is_err(),
            "{field} must no longer be accepted"
        );
    }
}

#[test]
fn multiplexer_defaults_and_invalid_kind() {
    let config = Config::from_toml(BASE).unwrap();
    assert_eq!(config.multiplexer.kind, agentix_core::MultiplexerKind::Rmux);
    assert_eq!(config.multiplexer.working_dir, dirs::home_dir().unwrap());
    assert!(Config::from_toml(&format!("{BASE}\n[multiplexer]\nkind='unknown'\n")).is_err());
}
