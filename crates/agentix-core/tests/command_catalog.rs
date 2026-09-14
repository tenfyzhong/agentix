#[test]
fn shared_command_catalog_includes_contextual_commands_without_platform_names() {
    let menu = agentix_core::command_menu(true);
    assert!(
        menu.commands
            .iter()
            .any(|command| command.name == "sessions")
    );
    assert!(menu.commands.iter().any(|command| command.name == "rename"));
    assert!(
        menu.commands
            .iter()
            .all(|command| !command.name.starts_with("agentix-"))
    );
    assert!(
        !agentix_core::command_menu(false)
            .commands
            .iter()
            .any(|command| command.name == "rename")
    );
}

#[test]
fn native_new_is_exposed_only_when_attached() {
    assert!(
        agentix_core::command_menu(true)
            .commands
            .iter()
            .any(|command| command.name == "new")
    );
    assert!(
        !agentix_core::command_menu(false)
            .commands
            .iter()
            .any(|command| command.name == "new")
    );
}

#[test]
fn last_is_a_contextual_command_only_when_attached() {
    let menu = agentix_core::command_menu(true);
    assert!(
        menu.commands
            .iter()
            .any(|command| command.name == "last" && command.contextual)
    );
    assert!(
        !agentix_core::command_menu(false)
            .commands
            .iter()
            .any(|command| command.name == "last")
    );
    assert!(agentix_core::parse_input("/last@agentix").is_ok());
}
