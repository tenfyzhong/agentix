use agentix_domain::TerminalInteractionKind;
use agentix_multiplexer::parse_terminal_interaction;

#[test]
fn terminal_choices_do_not_depend_on_command_title_or_option_names() {
    for marker in ["›", "❯", ">"] {
        let screen = format!(
            "Choose an unfamiliar deployment target\n\n{marker} 1. Sandbox    Isolated environment\n  2. Preview    Shared environment\n  3. Production    Customer environment\n\n  Enter to select · Esc to cancel\n"
        );
        let prompt = parse_terminal_interaction(&screen, 2).unwrap();
        assert_eq!(prompt.kind, TerminalInteractionKind::Choice);
        assert_eq!(prompt.title, "Choose an unfamiliar deployment target");
        assert_eq!(prompt.choices.len(), 3);
        assert_eq!(prompt.selected, Some(0));
        let moved = parse_terminal_interaction(
            &screen
                .replace(&format!("{marker} 1."), "  1.")
                .replace("  2.", &format!("{marker} 2.")),
            3,
        )
        .unwrap();
        assert_eq!(moved.fingerprint, prompt.fingerprint);
        assert_eq!(moved.selected, Some(1));
    }
}

#[test]
fn terminal_confirmation_is_explicit_and_unknown_dialogs_remain_visible() {
    let prompt = parse_terminal_interaction("Remove this temporary directory? [y/N]\n", 0).unwrap();
    assert_eq!(prompt.kind, TerminalInteractionKind::Confirmation);
    assert_eq!(prompt.choices, ["Yes", "No"]);
    let unknown = parse_terminal_interaction(
        "Custom permission editor\n [x] Network [ ] Files\n\n Enter to submit · Esc to cancel\n",
        1,
    )
    .unwrap();
    assert_eq!(unknown.kind, TerminalInteractionKind::Unknown);
    assert!(unknown.detail.contains("Custom permission editor"));
    assert!(unknown.choices.is_empty());
}

#[test]
fn terminal_output_and_drafts_are_not_interaction_requests() {
    for (screen, cursor) in [
        (
            "Recommended choices:\n› 1. Example\n  2. Example\n\n› local draft\n\n  model high · /tmp/repo\n",
            5,
        ),
        ("Working...\n1. First\n2. Second\n", 2),
        ("› Ask an agent a question\n", 0),
        ("Logs: Continue? [y/N]\nmore output\n", 1),
    ] {
        assert!(
            parse_terminal_interaction(screen, cursor).is_none(),
            "{screen}"
        );
    }
}

#[test]
fn arrow_menus_use_the_same_choice_flow_without_numbered_items() {
    let screen = "────────────\n\n Choose a future action\n\n → Continue\n   Open settings\n   Cancel operation\n\n ↑↓ navigate  enter select  esc cancel\n\n────────────\n";
    let prompt = parse_terminal_interaction(screen, 4).unwrap();
    assert_eq!(prompt.kind, TerminalInteractionKind::Choice);
    assert_eq!(prompt.title, "Choose a future action");
    assert_eq!(
        prompt.choices,
        ["1. Continue", "2. Open settings", "3. Cancel operation"]
    );
    let moved = parse_terminal_interaction(
        &screen
            .replace(" → Continue", "   Continue")
            .replace("   Open settings", " → Open settings"),
        5,
    )
    .unwrap();
    assert_eq!(prompt.fingerprint, moved.fingerprint);
    assert_eq!(moved.selected, Some(1));
}

#[test]
fn choice_menus_can_park_the_cursor_in_their_footer() {
    let screen = "Welcome\n\n Future command choices\n\n› 1. First\n  2. Second\n\n enter select · esc back\n";
    let prompt = parse_terminal_interaction(screen, 7).unwrap();
    assert_eq!(prompt.kind, TerminalInteractionKind::Choice);
    assert_eq!(prompt.title, "Future command choices");
    assert_eq!(prompt.choices, ["1. First", "2. Second"]);
}
