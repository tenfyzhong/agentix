use agentix::Config;

const BASE: &str = "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[storage]\npath='/tmp/state'\n[agent]\nkind='codex'\n";

#[test]
fn accepts_global_multiplexer_configuration() {
    let config = Config::from_toml(&format!("{BASE}\n[multiplexer]\nkind='tmux'\n"));
    let config = config.unwrap();
    assert_eq!(config.multiplexer.kind, agentix::MultiplexerMode::Tmux);
    assert_eq!(config.multiplexer.home_dir, dirs::home_dir().unwrap());
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
    assert_eq!(config.multiplexer.home_dir, dirs::home_dir().unwrap());
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
                async { false },
            )
            .await;
        assert_eq!(resolved, expected, "{mode:?}, {native}, {compatible}");
        let expected_calls = match mode {
            agentix::MultiplexerMode::Rex => vec!["rex"],
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

#[test]
fn removed_working_directory_is_rejected() {
    let error =
        Config::from_toml(&format!("{BASE}\n[multiplexer]\nworking_dir='~/work'\n")).unwrap_err();
    assert!(format!("{error:#}").contains("working_dir"));
}

#[test]
fn rex_configuration_and_menu_are_available() {
    let config = Config::from_toml(&format!("{BASE}\n[multiplexer]\nkind='rex'\n")).unwrap();
    assert_eq!(format!("{:?}", config.multiplexer.kind), "Rex");
    let menu = agentix_core::command_menu_for(false, agentix_core::MultiplexerKind::Rex);
    assert!(menu.commands.iter().any(|entry| entry.name == "rex"));
    assert!(
        !menu
            .commands
            .iter()
            .any(|entry| entry.name == "tmux" || entry.name == "rmux")
    );
}

#[tokio::test]
async fn rex_detection_keeps_existing_priority_and_explicit_mode_does_not_fall_back() {
    use agentix::MultiplexerMode;
    use agentix_core::MultiplexerKind;
    for rmux in [false, true] {
        for tmux in [false, true] {
            for rex in [false, true] {
                let resolved = MultiplexerMode::Auto
                    .resolve_with(async { rmux }, async { tmux }, async { rex })
                    .await;
                assert_eq!(
                    resolved,
                    if rmux {
                        Some(MultiplexerKind::Rmux)
                    } else if tmux {
                        Some(MultiplexerKind::Tmux)
                    } else if rex {
                        Some(MultiplexerKind::Rex)
                    } else {
                        None
                    }
                );
                let explicit = MultiplexerMode::Rex
                    .resolve_with(
                        async { panic!("rmux must not be polled") },
                        async { panic!("tmux must not be polled") },
                        async { rex },
                    )
                    .await;
                assert_eq!(explicit, rex.then_some(MultiplexerKind::Rex));
            }
        }
    }
}
