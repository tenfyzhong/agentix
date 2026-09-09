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
