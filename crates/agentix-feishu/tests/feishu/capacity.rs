use super::*;
use agentix_domain::ViewSection;
use serde_json::Value;

async fn fixture() -> (MockFeishuApi, FeishuAdapter, ConversationRef) {
    let server = MockFeishuApi::start().await;
    let client = LarkClient::builder("capacity-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    (
        server,
        adapter,
        ConversationRef::new(ChannelKind::Feishu, "oc_capacity"),
    )
}

fn section(title: &str, body: &str, collapsible: bool) -> ViewSection {
    ViewSection {
        title: title.into(),
        body: body.into(),
        collapsible,
        expanded: Some(!collapsible),
        ..ViewSection::default()
    }
}

async fn writes(server: &MockFeishuApi) -> Vec<support::CapturedRequest> {
    server
        .requests()
        .await
        .into_iter()
        .filter(|request| {
            request.target.starts_with("/open-apis/im/v1/messages")
                && matches!(request.method.as_str(), "POST" | "PATCH" | "DELETE")
        })
        .collect()
}

fn card(request: &support::CapturedRequest) -> Value {
    let body: Value = serde_json::from_str(&request.body).unwrap();
    serde_json::from_str(body["content"].as_str().unwrap()).unwrap()
}

fn markdown(value: &Value) -> String {
    if value["tag"] == "markdown" {
        return value["content"].as_str().unwrap().to_owned();
    }
    match value {
        Value::Object(object) => object.values().map(markdown).collect(),
        Value::Array(items) => items.iter().map(markdown).collect(),
        _ => String::new(),
    }
}

#[tokio::test]
async fn capacity_short_sections_share_one_card_without_per_section_truncation() {
    let (server, adapter, conversation) = fixture().await;
    let answer = "a".repeat(15_000);
    let mut view = OutboundView::text("Codex", "fallback");
    view.sections = vec![
        section("Reasoning", "thinking", true),
        section("Tool", "done", true),
        section("", &answer, false),
    ];
    adapter.send(&conversation, &view).await.unwrap();
    let requests = writes(&server).await;
    assert_eq!(requests.len(), 1);
    let card = card(&requests[0]);
    assert_eq!(card["body"]["elements"][0]["expanded"], false);
    assert_eq!(card["body"]["elements"][1]["expanded"], false);
    assert_eq!(card["body"]["elements"][2]["content"], answer);
}

#[tokio::test]
async fn capacity_long_unicode_output_is_lossless_and_actions_stay_on_anchor() {
    let (server, adapter, conversation) = fixture().await;
    let mut view = OutboundView::text("Codex", "中文🧠 paragraph\n\n".repeat(5_000));
    view.actions.push(ActionButton {
        label: "Stop".into(),
        token: "stop-token".into(),
        style: ActionStyle::Danger,
        disabled: false,
    });
    adapter.send(&conversation, &view).await.unwrap();
    let requests = writes(&server).await;
    assert!(requests.len() > 1);
    assert_eq!(
        requests
            .iter()
            .map(|request| markdown(&card(request)))
            .collect::<String>(),
        view.body
    );
    for (index, request) in requests.iter().enumerate() {
        assert!(request.body.len() < 30_000);
        assert_eq!(request.body.contains("stop-token"), index == 0);
    }
}

#[tokio::test]
async fn capacity_accounts_for_json_escaping_and_keeps_empty_short_views() {
    let (server, adapter, conversation) = fixture().await;
    let view = OutboundView::text("Escapes", "\"\\\n\t".repeat(8_000));
    adapter.send(&conversation, &view).await.unwrap();
    let requests = writes(&server).await;
    assert!(requests.len() > 1);
    assert!(requests.iter().all(|request| request.body.len() < 30_000));
    assert_eq!(
        requests
            .iter()
            .map(|request| markdown(&card(request)))
            .collect::<String>(),
        view.body
    );
    adapter
        .send(&conversation, &OutboundView::text("Empty", ""))
        .await
        .unwrap();
    assert_eq!(writes(&server).await.len(), requests.len() + 1);
}

#[tokio::test]
async fn capacity_growth_reuses_cards_and_identical_updates_send_nothing() {
    let (server, adapter, conversation) = fixture().await;
    let mut view = OutboundView::text("Codex", "short");
    let message = adapter.send(&conversation, &view).await.unwrap();
    view.body = "paragraph\n\n".repeat(8_000);
    adapter
        .update(&conversation, &message, &view)
        .await
        .unwrap();
    let requests = writes(&server).await;
    assert!(requests.iter().filter(|r| r.method == "POST").count() > 1);
    assert_eq!(requests[1].method, "PATCH");
    adapter
        .update(&conversation, &message, &view)
        .await
        .unwrap();
    assert_eq!(writes(&server).await.len(), requests.len());
}

#[tokio::test]
async fn capacity_shrink_removes_obsolete_continuations() {
    let (server, adapter, conversation) = fixture().await;
    let mut view = OutboundView::text("Codex", "paragraph\n\n".repeat(8_000));
    let message = adapter.send(&conversation, &view).await.unwrap();
    let pages = writes(&server).await.len();
    assert!(pages > 1);
    view.body = "revised short answer".into();
    adapter
        .update(&conversation, &message, &view)
        .await
        .unwrap();
    let requests = writes(&server).await;
    assert_eq!(
        requests.iter().filter(|r| r.method == "DELETE").count(),
        pages - 1
    );
    assert!(
        requests
            .iter()
            .filter(|r| r.method == "DELETE")
            .all(|r| !r.target.ends_with(&message.message_id))
    );
}

#[tokio::test]
async fn capacity_disable_preserves_every_page_and_retries_rejected_cleanup() {
    let (server, adapter, conversation) = fixture().await;
    let mut view = OutboundView::text("Codex", "chunk\n\n".repeat(10_000));
    view.actions.push(ActionButton {
        label: "Stop".into(),
        token: "stop-token".into(),
        style: ActionStyle::Danger,
        disabled: false,
    });
    let message = adapter.send(&conversation, &view).await.unwrap();
    let sent = writes(&server).await;
    assert!(sent.len() > 1);
    server
        .fail_next(
            &format!("/messages/{}", message.message_id),
            230_001,
            "rejected",
        )
        .await;
    assert!(adapter.disable_actions(&message).await.is_err());
    adapter.disable_actions(&message).await.unwrap();
    let requests = writes(&server).await;
    assert_eq!(requests.len(), sent.len() + 2);
    let disabled = card(requests.last().unwrap());
    assert_eq!(markdown(&disabled), markdown(&card(&sent[0])));
    assert_eq!(
        disabled["body"]["elements"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["disabled"],
        true
    );
    adapter.disable_actions(&message).await.unwrap();
    assert_eq!(writes(&server).await.len(), requests.len());
}

#[tokio::test]
async fn capacity_retry_of_rejected_update_does_not_resend_successful_pages() {
    let (server, adapter, conversation) = fixture().await;
    let mut view = OutboundView::text("Codex", "old\n\n".repeat(15_000));
    let message = adapter.send(&conversation, &view).await.unwrap();
    assert!(writes(&server).await.len() > 1);
    view.body = "new\n\n".repeat(15_000);
    server
        .fail_next("/messages/om_mock_message_1", 230_001, "rejected")
        .await;
    assert!(matches!(
        adapter.update(&conversation, &message, &view).await,
        Err(ChannelError::Rejected(_))
    ));
    let before_retry = writes(&server).await;
    adapter
        .update(&conversation, &message, &view)
        .await
        .unwrap();
    let after_retry = writes(&server).await;
    assert!(
        after_retry[before_retry.len()..].iter().all(
            |request| request.method != "POST" && !request.target.ends_with("/om_mock_message")
        )
    );
}

#[tokio::test]
async fn capacity_preserves_fitting_code_blocks_and_balances_oversized_fences() {
    for code in [
        "let answer = 42;\n".repeat(400),
        "let answer = 42;\n".repeat(5_000),
    ] {
        let (server, adapter, conversation) = fixture().await;
        let view = OutboundView::text(
            "Code",
            format!("{}\n\n```rust\n{code}```\n", "intro ".repeat(3_000)),
        );
        adapter.send(&conversation, &view).await.unwrap();
        let requests = writes(&server).await;
        let mut delivered_code = String::new();
        let mut code_pages = 0;
        for request in requests {
            let text = markdown(&card(&request));
            let mut in_code = false;
            for line in text.lines() {
                if line.starts_with("```") {
                    in_code = !in_code;
                    if in_code {
                        code_pages += 1;
                    }
                } else if in_code {
                    delivered_code.push_str(line);
                    delivered_code.push('\n');
                }
            }
            assert!(!in_code, "each card must close its code fence");
        }
        assert_eq!(delivered_code, code);
        if code.len() < 20_000 {
            assert_eq!(code_pages, 1);
        }
    }
}

#[tokio::test]
async fn capacity_elapsed_only_updates_do_not_rewrite_completed_pages() {
    let (server, adapter, conversation) = fixture().await;
    let mut view = OutboundView::text("Codex", "paragraph\n\n".repeat(8_000));
    view.status = ViewStatus::Running;
    view.subtitle = Some("Running · 1s".into());
    let message = adapter.send(&conversation, &view).await.unwrap();
    let before = writes(&server).await.len();
    assert!(before > 1);
    view.subtitle = Some("Running · 2s".into());
    adapter
        .update(&conversation, &message, &view)
        .await
        .unwrap();
    let requests = writes(&server).await;
    assert_eq!(requests.len(), before + 1);
    assert!(
        requests
            .last()
            .unwrap()
            .target
            .ends_with(&format!("om_mock_message_{}", before - 1))
    );
}

#[tokio::test]
async fn capacity_initial_partial_rejection_retracts_confirmed_pages() {
    let (server, adapter, conversation) = fixture().await;
    let release = server.hold_next("/open-apis/im/v1/messages?").await;
    let operation = tokio::spawn(async move {
        adapter
            .send(
                &conversation,
                &OutboundView::text("Long", "paragraph\n\n".repeat(8_000)),
            )
            .await
    });
    wait_for_feishu_message_request(&server).await;
    server
        .fail_next(
            "/open-apis/im/v1/messages?",
            230_001,
            "rejected continuation",
        )
        .await;
    release.send(()).unwrap();
    assert!(matches!(
        operation.await.unwrap(),
        Err(ChannelError::Rejected(_))
    ));
    let requests = writes(&server).await;
    assert_eq!(
        requests
            .iter()
            .map(|r| r.method.as_str())
            .collect::<Vec<_>>(),
        ["POST", "POST", "DELETE"]
    );
    assert!(requests[2].target.ends_with("om_mock_message"));
}

#[tokio::test]
async fn capacity_each_send_has_a_distinct_idempotency_key_stable_across_refresh() {
    let (server, adapter, conversation) = fixture().await;
    server
        .fail_next(
            "/open-apis/im/v1/messages?",
            99_991_663,
            "Invalid tenant_access_token",
        )
        .await;
    adapter
        .send(
            &conversation,
            &OutboundView::text("Long", "paragraph\n\n".repeat(8_000)),
        )
        .await
        .unwrap();
    let requests = writes(&server).await;
    let ids: Vec<String> = requests
        .iter()
        .map(|r| {
            let body: Value = serde_json::from_str(&r.body).unwrap();
            body["uuid"]
                .as_str()
                .expect("send requires an idempotency key")
                .to_owned()
        })
        .collect();
    assert!(ids.len() > 2);
    assert_eq!(ids[0], ids[1]);
    assert_eq!(
        ids[1..]
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        ids.len() - 1
    );
}

#[tokio::test]
async fn capacity_card_group_holds_the_conversation_fifo_until_complete() {
    let (server, adapter, conversation) = fixture().await;
    let release = server.hold_next("/open-apis/im/v1/messages?").await;
    let first_adapter = adapter.clone();
    let first_conversation = conversation.clone();
    let first = tokio::spawn(async move {
        first_adapter
            .send(
                &first_conversation,
                &OutboundView::text("Long", "paragraph\n\n".repeat(8_000)),
            )
            .await
    });
    wait_for_feishu_message_request(&server).await;
    let second = tokio::spawn(async move {
        adapter
            .send(&conversation, &OutboundView::text("Next", "next message"))
            .await
    });
    release.send(()).unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    let requests = writes(&server).await;
    assert!(requests.len() > 2);
    assert_eq!(markdown(&card(requests.last().unwrap())), "next message");
    assert!(
        requests[..requests.len() - 1]
            .iter()
            .all(|request| !request.body.contains("next message"))
    );
}

#[test]
fn capacity_many_small_panels_preserve_order_and_original_expansion() {
    let mut view = OutboundView::text("Panels", "unused");
    for index in 0..300 {
        view.sections.push(ViewSection {
            title: format!("Panel {index}"),
            body: format!("Content {index}\n"),
            collapsible: true,
            ..ViewSection::default()
        });
    }
    let cards = agentix_feishu::render_cards(&view).unwrap();
    assert!(cards.len() > 1);
    let mut index = 0;
    for card in cards {
        let json = serde_json::to_value(card.card()).unwrap();
        for element in json["body"]["elements"].as_array().unwrap() {
            assert_eq!(
                element["elements"][0]["content"],
                format!("Content {index}\n")
            );
            assert_eq!(element["expanded"], index == 299);
            index += 1;
        }
    }
    assert_eq!(index, 300);
}

#[tokio::test]
async fn capacity_rejects_oversized_metadata_before_any_message_is_sent() {
    let (server, adapter, conversation) = fixture().await;
    let view = OutboundView::text("title".repeat(8_000), "body");
    assert!(matches!(
        adapter.send(&conversation, &view).await,
        Err(ChannelError::InvalidPayload(_))
    ));
    assert!(writes(&server).await.is_empty());
}
