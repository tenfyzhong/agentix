use agentix_domain::{ActionButton, ActionStyle, OutboundView};
use agentix_slack::render_view;

#[test]
fn blocks_preserve_code_and_escape_mentions_and_offer_buttons() {
    let mut view = OutboundView::text("Title <&>", "**Bold**\n```rust\nlet x = 1;\n```\n<@U1>");
    view.actions.push(ActionButton {
        disabled: false,
        label: "Choose".into(),
        token: "token".into(),
        style: ActionStyle::Primary,
    });
    let payload = render_view(&view).unwrap();
    assert!(payload["text"].as_str().unwrap().contains("Title"));
    let blocks = payload["blocks"].as_array().unwrap();
    assert_eq!(blocks[0]["text"]["text"], "Title <&>");
    let body = blocks[1]["text"]["text"].as_str().unwrap();
    assert!(body.contains("```"));
    assert!(body.contains("&lt;@U1&gt;"));
    assert_eq!(blocks.last().unwrap()["elements"][0]["value"], "token");
}

#[test]
fn large_unicode_views_remain_within_slack_block_limits() {
    let view = OutboundView::text("😀".repeat(200), "中文😀".repeat(30_000));
    let payload = render_view(&view).unwrap();
    let blocks = payload["blocks"].as_array().unwrap();
    assert!(blocks.len() <= 50);
    assert!(blocks[0]["text"]["text"].as_str().unwrap().chars().count() <= 150);
    for block in &blocks[1..] {
        assert!(block["text"]["text"].as_str().unwrap().chars().count() <= 3000);
    }
    assert!(payload["text"].as_str().unwrap().chars().count() <= 4000);
}

#[test]
fn invalid_action_tokens_are_rejected_instead_of_silently_corrupted() {
    let mut view = OutboundView::text("Title", "Body");
    view.actions.push(ActionButton {
        disabled: false,
        label: "Go".into(),
        token: "x".repeat(2001),
        style: ActionStyle::Default,
    });
    assert!(render_view(&view).is_err());
}

#[test]
fn notification_fallback_cannot_ping_members_or_broadcast_mentions() {
    let view = OutboundView::text("<@U1>", "<!channel> & <https://example.com|x>");
    let payload = render_view(&view).unwrap();
    let fallback = payload["text"].as_str().unwrap();
    assert!(!fallback.contains('<'));
    assert!(fallback.contains("&lt;!channel&gt;"));
}

#[test]
fn escaping_never_splits_entities_between_sections() {
    let view = OutboundView::text("Title", format!("{}<&>", "x".repeat(2899)));
    let payload = render_view(&view).unwrap();
    for block in payload["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "section")
    {
        let text = block["text"]["text"].as_str().unwrap();
        if let Some((_, tail)) = text.rsplit_once('&') {
            assert!(tail.contains(';'), "split entity: {tail}");
        }
    }
}

#[test]
fn chunked_code_fences_remain_balanced_and_bold_is_converted_outside_code() {
    let view = OutboundView::text(
        "Title",
        format!(
            "**Bold**\n```\n{}\nlet stars = \"**literal**\";\n```",
            "x".repeat(6000)
        ),
    );
    let payload = render_view(&view).unwrap();
    let sections: Vec<_> = payload["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "section")
        .map(|block| block["text"]["text"].as_str().unwrap())
        .collect();
    assert!(sections[0].starts_with("*Bold*"));
    assert!(
        sections
            .iter()
            .all(|section| section.matches("```").count() % 2 == 0)
    );
    assert!(sections.join("").contains("**literal**"));
}

#[test]
fn disabled_buttons_do_not_emit_clickable_actions() {
    let mut view = agentix_domain::OutboundView::text("Sessions", "Current session");
    view.actions = serde_json::from_value(serde_json::json!([
        {"label":"Attached", "token":"display-only", "style":"default", "disabled":true}
    ]))
    .unwrap();
    assert!(
        render_view(&view).unwrap()["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|block| block["type"] != "actions")
    );
}

#[test]
fn structured_entry_buttons_follow_descriptions_and_footer_comes_last() {
    let mut view = OutboundView::text("Jobs", "Flat fallback");
    for (label, token, style) in [
        ("First", "a", ActionStyle::Default),
        ("Second", "b", ActionStyle::Default),
        ("ACTIVE", "filter", ActionStyle::Primary),
    ] {
        view.actions.push(ActionButton {
            disabled: false,
            label: label.into(),
            token: token.into(),
            style,
        });
        view.sections.push(agentix_domain::ViewSection {
            title: label.into(),
            body: format!("Description for {label}"),
            action_tokens: vec![token.into()],
            ..Default::default()
        });
    }
    let payload = render_view(&view).unwrap();
    let blocks = payload["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 7);
    for (index, token) in ["a", "b", "filter"].into_iter().enumerate() {
        assert_eq!(blocks[1 + index * 2]["type"], "section");
        assert_eq!(blocks[2 + index * 2]["elements"][0]["value"], token);
    }
    assert_eq!(blocks[6]["elements"][0]["style"], "primary");
}

#[test]
fn section_layout_deduplicates_tokens_and_keeps_unplaced_actions_at_the_end() {
    let mut view = OutboundView::text("Jobs", "Fallback");
    for (token, disabled) in [("a", false), ("disabled", true), ("unplaced", false)] {
        view.actions.push(ActionButton {
            disabled,
            label: token.into(),
            token: token.into(),
            style: ActionStyle::Default,
        });
    }
    for title in ["First", "Second"] {
        view.sections.push(agentix_domain::ViewSection {
            title: title.into(),
            body: "Description".into(),
            action_tokens: vec!["a".into(), "unknown".into(), "disabled".into()],
            ..Default::default()
        });
    }
    let value = render_view(&view).unwrap();
    let buttons: Vec<_> = value["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "actions")
        .flat_map(|b| b["elements"].as_array().unwrap())
        .map(|b| b["value"].as_str().unwrap())
        .collect();
    assert_eq!(buttons, ["a", "unplaced"]);
}

#[test]
fn section_layout_reserves_space_for_every_footer_control() {
    let mut view = OutboundView::text("Jobs", "Fallback");
    for index in 0..8 {
        let token = format!("token-{index}");
        view.actions.push(ActionButton {
            disabled: false,
            label: token.clone(),
            token: token.clone(),
            style: ActionStyle::Primary,
        });
        view.sections.push(agentix_domain::ViewSection {
            title: index.to_string(),
            body: "\u{957f}\u{5185}\u{5bb9}".repeat(10000),
            action_tokens: vec![token],
            ..Default::default()
        });
    }
    let value = render_view(&view).unwrap();
    let blocks = value["blocks"].as_array().unwrap();
    assert!(blocks.len() <= 50);
    assert_eq!(blocks.iter().filter(|b| b["type"] == "actions").count(), 8);
    assert_eq!(blocks.last().unwrap()["elements"][0]["value"], "token-7");
}

#[test]
fn excessive_sections_keep_the_flat_fallback_and_all_controls() {
    let mut view = OutboundView::text("Job", "Complete bounded summary");
    view.sections = (0..60)
        .map(|_| agentix_domain::ViewSection {
            body: "Detail".into(),
            ..Default::default()
        })
        .collect();
    view.actions.push(ActionButton {
        disabled: false,
        label: "Job".into(),
        token: "job".into(),
        style: ActionStyle::Default,
    });
    let payload = render_view(&view).unwrap();
    assert!(payload["blocks"].as_array().unwrap().len() <= 50);
    assert!(
        payload["blocks"][1]["text"]["text"]
            .as_str()
            .unwrap()
            .contains("Complete bounded summary")
    );
    assert_eq!(
        payload["blocks"].as_array().unwrap().last().unwrap()["elements"][0]["value"],
        "job"
    );
}
