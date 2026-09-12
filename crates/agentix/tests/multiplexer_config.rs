use agentix::Config;

const BASE: &str = "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[storage]\npath='/tmp/state'\n[agent]\nkind='codex'\n";

#[test]
fn accepts_global_multiplexer_configuration() {
    let config = Config::from_toml(&format!(
        "{BASE}\n[multiplexer]\nkind='tmux'\nworking_dir='~/work'\n"
    ));
    let config = config.unwrap();
    assert_eq!(config.multiplexer.kind, agentix::MultiplexerMode::Tmux);
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
    assert_eq!(config.multiplexer.kind, agentix::MultiplexerMode::Auto);
    assert_eq!(config.multiplexer.working_dir, dirs::home_dir().unwrap());
    assert!(Config::from_toml(&format!("{BASE}\n[multiplexer]\nkind='unknown'\n")).is_err());
}

#[test]
fn automatic_multiplexer_is_the_default_and_can_be_explicit() {
    let implicit = Config::from_toml(BASE).unwrap();
    assert_eq!(format!("{:?}", implicit.multiplexer.kind), "Auto");
    let explicit = Config::from_toml(&format!("{BASE}\n[multiplexer]\nkind='auto'\n")).unwrap();
    assert_eq!(implicit.multiplexer, explicit.multiplexer);
}

#[tokio::test]
async fn configured_mode_resolves_only_successful_probes() {
    use agentix::MultiplexerMode::{Auto, Rmux, Tmux};
    use agentix_core::MultiplexerKind;
    for (mode, native, compatible, expected) in [
        (Auto, false, false, None),
        (Auto, false, true, Some(MultiplexerKind::Tmux)),
        (Auto, true, false, Some(MultiplexerKind::Rmux)),
        (Auto, true, true, Some(MultiplexerKind::Rmux)),
        (Rmux, false, false, None),
        (Rmux, false, true, None),
        (Rmux, true, false, Some(MultiplexerKind::Rmux)),
        (Rmux, true, true, Some(MultiplexerKind::Rmux)),
        (Tmux, false, false, None),
        (Tmux, true, false, None),
        (Tmux, false, true, Some(MultiplexerKind::Tmux)),
        (Tmux, true, true, Some(MultiplexerKind::Tmux)),
    ] {
        let calls = std::cell::RefCell::new(Vec::new());
        let resolved = mode
            .resolve_with(
                async {
                    calls.borrow_mut().push("rmux");
                    native
                },
                async {
                    calls.borrow_mut().push("tmux");
                    compatible
                },
            )
            .await;
        assert_eq!(resolved, expected, "{mode:?}, {native}, {compatible}");
        let expected_calls = match mode {
            Tmux => vec!["tmux"],
            Auto if !native => vec!["rmux", "tmux"],
            _ => vec!["rmux"],
        };
        assert_eq!(*calls.borrow(), expected_calls);
        let menu = agentix_core::command_menu_for(false, resolved);
        let mux_commands: Vec<_> = menu
            .commands
            .iter()
            .filter(|c| c.name == "rmux" || c.name == "tmux")
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            mux_commands,
            expected
                .into_iter()
                .map(MultiplexerKind::as_str)
                .collect::<Vec<_>>()
        );
        let telegram = agentix_telegram::menu_commands_for(resolved);
        assert_eq!(
            telegram
                .iter()
                .filter(|c| c.command == "rmux" || c.command == "tmux")
                .count(),
            usize::from(expected.is_some())
        );
    }
}
