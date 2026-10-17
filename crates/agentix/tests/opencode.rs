use agentix::Config;
use agentix_core::{SessionId, SessionRef};

#[test]
fn opencode_named_backend_defaults_and_qualified_identity() {
    let config = Config::from_toml(
        "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[storage]\npath='/tmp/state.db'\n[agent.opencode]\n",
    ).unwrap();
    let kind = config.selected_agents()[0].kind();
    assert_eq!(kind.as_str(), "opencode");
    assert_eq!(kind.display_name(), "OpenCode");
    let reference = SessionRef::decode(&SessionId::new("opencode:ses_opaque:part")).unwrap();
    assert_eq!(reference.agent, kind);
    assert_eq!(reference.native_id.as_str(), "ses_opaque:part");
    assert_eq!(reference.encode().as_str(), "opencode:ses_opaque:part");
}

#[test]
fn opencode_state_directory_expands_and_configuration_rejects_unknown_fields() {
    let base = "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[storage]\npath='/tmp/state.db'\n[agent.opencode]\n";
    let config = Config::from_toml(&format!(
        "{base}command='~/bin/opencode'\nstate_dir='~/bridge/opencode'\n"
    ))
    .unwrap();
    let debug = format!("{:?}", config.selected_agents()[0]);
    assert!(debug.contains("/bin/opencode"));
    assert!(!debug.contains("~/"));
    assert!(Config::from_toml(&format!("{base}endpoint='http://localhost:4096'\n")).is_err());
}
