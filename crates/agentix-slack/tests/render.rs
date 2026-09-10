use agentix_domain::{ActionButton, ActionStyle, OutboundView};
use agentix_slack::render_view;

#[test]
fn blocks_preserve_code_and_escape_mentions_and_offer_buttons() {
    let mut view = OutboundView::text("Title <&>", "**Bold**\n```rust\nlet x = 1;\n```\n<@U1>");
    view.actions.push(ActionButton {
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
