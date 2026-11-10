use agentix::Config;
use agentix_core::{SessionId, SessionRef};

#[test]
fn antigravity_named_backend_defaults_and_qualified_identity() {
    let config = Config::from_toml(
        "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[storage]\npath='/tmp/state.db'\n[agent.antigravity]\n",
    ).unwrap();
    let kind = config.selected_agents()[0].kind();
    assert_eq!(kind.as_str(), "antigravity");
    assert_eq!(kind.display_name(), "Antigravity");
    let reference = SessionRef::decode(&SessionId::new("antigravity:ses_opaque:part")).unwrap();
    assert_eq!(reference.agent, kind);
    assert_eq!(reference.native_id.as_str(), "ses_opaque:part");
    assert_eq!(reference.encode().as_str(), "antigravity:ses_opaque:part");
}

#[test]
fn antigravity_state_directory_expands_and_configuration_rejects_unknown_fields() {
    let base = "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[storage]\npath='/tmp/state.db'\n[agent.antigravity]\n";
    let config = Config::from_toml(&format!(
        "{base}command='~/bin/antigravity'\nstate_dir='~/bridge/antigravity'\n"
    ))
    .unwrap();
    let debug = format!("{:?}", config.selected_agents()[0]);
    assert!(debug.contains("/bin/antigravity"));
    assert!(!debug.contains("~/"));
    assert!(Config::from_toml(&format!("{base}endpoint='http://localhost:4096'\n")).is_err());
}

#[test]
fn antigravity_expands_all_paths_and_defaults_to_agy() {
    let base = "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[storage]\npath='/tmp/state.db'\n[agent.antigravity]\n";
    let config = Config::from_toml(base).unwrap();
    let value = format!("{:?}", config.selected_agents()[0]);
    assert!(value.contains("command: \"agy\""));
    assert!(value.contains("/.gemini/antigravity-cli/brain"));
    assert!(value.contains("/.local/share/agentix/antigravity"));
    let config = Config::from_toml(&format!(
        "{base}command='~/bin/agy'\nsession_dir='~/sessions'\nstate_dir='~/state'\n"
    ))
    .unwrap();
    let value = format!("{:?}", config.selected_agents()[0]);
    assert!(!value.contains("~/"));
    assert!(value.contains("/sessions"));
    assert!(value.contains("/state"));
}
