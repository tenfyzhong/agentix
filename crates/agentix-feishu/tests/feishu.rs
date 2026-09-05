mod support;

use std::sync::{Arc, Mutex};

use agentix_core::{
    ActionButton, ActionStyle, ChannelAdapter, ChannelCommand, ChannelError, ChannelKind,
    CommandMenu, ConversationRef, MessageRef, OutboundView, ViewStatus,
};
use agentix_feishu::{
    FeishuAdapter, FeishuOwnerClaimer, FeishuPolicy, render_card, strip_bot_mentions,
};
use async_trait::async_trait;
use larksuite_oapi_sdk_rs::LarkClient;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use support::MockFeishuApi;

fn authorization_header(request: &support::CapturedRequest) -> &str {
    request
        .headers
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
        .expect("request must include an authorization header")
}

#[derive(Default)]
struct RecordingOwnerClaimer {
    claims: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl FeishuOwnerClaimer for RecordingOwnerClaimer {
    async fn claim(&self, code: &str, owner_open_id: &str) -> Result<bool, String> {
        self.claims
            .lock()
            .unwrap()
            .push((code.into(), owner_open_id.into()));
        Ok(true)
    }
}

#[test]
fn policy_accepts_only_owners_and_requires_group_mentions() {
    let policy = FeishuPolicy::new(["ou_owner"]);

    assert!(policy.accept("ou_owner", true, false));
    assert!(policy.accept("ou_owner", false, true));
    assert!(!policy.accept("ou_owner", false, false));
    assert!(!policy.accept("ou_stranger", true, true));
}

#[test]
fn card_uses_v2_shared_schema_and_opaque_callback_tokens() {
    let view = OutboundView {
        title: "Codex · 12345678".into(),
        subtitle: Some("Turn turn-7 · running".into()),
        body: "Compiling the workspace".into(),
        status: ViewStatus::Running,
        actions: vec![ActionButton {
            label: "Stop".into(),
            token: "opaque-token".into(),
            style: ActionStyle::Danger,
        }],
    };

    let card = render_card(&view).unwrap();
    let value = card.card().to_json().into_value();

    assert_eq!(value["schema"], "2.0");
    assert_eq!(value["config"]["update_multi"], true);
    assert_eq!(value["header"]["title"]["content"], "Codex · 12345678");
    assert_eq!(
        value["header"]["subtitle"]["content"],
        "Turn turn-7 · running"
    );
    assert_eq!(value["header"]["template"], "blue");
    assert_eq!(value["body"]["elements"][0]["tag"], "markdown");
    assert_eq!(value["body"]["elements"][1]["tag"], "button");
    assert_eq!(
        value["body"]["elements"][1]["behaviors"][0]["value"],
        serde_json::json!({"token": "opaque-token"})
    );
    assert!(value.to_string().contains("Compiling the workspace"));
}

#[test]
fn strips_feishu_bot_placeholders_before_forwarding_prompts() {
    assert_eq!(
        strip_bot_mentions("@_user_1 please run tests", ["@_user_1"]),
        "please run tests"
    );
}

#[tokio::test]
async fn feishu_send_and_update_use_the_mock_openapi() {
    let server = MockFeishuApi::start().await;
    let client = LarkClient::builder("mock-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let conversation = ConversationRef::new(ChannelKind::Feishu, "oc_mock_chat");
    let initial = OutboundView {
        title: "Approval".into(),
        subtitle: Some("Waiting".into()),
        body: "Allow **cargo test**?".into(),
        status: ViewStatus::Waiting,
        actions: vec![ActionButton {
            label: "Allow once".into(),
            token: "opaque-token".into(),
            style: ActionStyle::Primary,
        }],
    };

    let message = adapter.send(&conversation, &initial).await.unwrap();
    assert_eq!(message.message_id, "om_mock_message");
    adapter.disable_actions(&message).await.unwrap();
    adapter
        .update(
            &conversation,
            &MessageRef::new(conversation.clone(), "om_mock_message"),
            &OutboundView::text("Approval", "Selected: Allow once"),
        )
        .await
        .unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].method, "POST");
    assert!(
        requests[0]
            .target
            .starts_with("/open-apis/auth/v3/tenant_access_token/internal")
    );
    assert_eq!(requests[1].method, "POST");
    assert!(
        requests[1]
            .target
            .starts_with("/open-apis/im/v1/messages?receive_id_type=chat_id")
    );
    assert!(
        requests[1]
            .headers
            .contains("authorization: Bearer mock-tenant-token-1")
    );
    let sent: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(sent["receive_id"], "oc_mock_chat");
    assert_eq!(sent["msg_type"], "interactive");
    let sent_card: serde_json::Value =
        serde_json::from_str(sent["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        sent_card["body"]["elements"][1]["behaviors"][0]["value"]["token"],
        "opaque-token"
    );
    assert_eq!(requests[2].method, "PATCH");
    assert_eq!(
        requests[2].target,
        "/open-apis/im/v1/messages/om_mock_message"
    );
    let disabled: serde_json::Value = serde_json::from_str(&requests[2].body).unwrap();
    let disabled_card: serde_json::Value =
        serde_json::from_str(disabled["content"].as_str().unwrap()).unwrap();
    assert_eq!(disabled_card["body"]["elements"][1]["disabled"], true);
    assert_eq!(requests[3].method, "PATCH");
    assert_eq!(
        requests[3].target,
        "/open-apis/im/v1/messages/om_mock_message"
    );
    let updated: serde_json::Value = serde_json::from_str(&requests[3].body).unwrap();
    let updated_card: serde_json::Value =
        serde_json::from_str(updated["content"].as_str().unwrap()).unwrap();
    assert!(!updated_card.to_string().contains("opaque-token"));
}

#[tokio::test]
async fn feishu_command_menu_is_sent_once_and_updated_for_attached_sessions() {
    let server = MockFeishuApi::start().await;
    let client = LarkClient::builder("mock-menu-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let conversation = ConversationRef::new(ChannelKind::Feishu, "oc_mock_chat");
    let detached = CommandMenu::new(vec![ChannelCommand::new(
        "sessions",
        "Browse running sessions",
    )]);
    let attached = CommandMenu::new(vec![
        ChannelCommand::new("sessions", "Browse running sessions"),
        ChannelCommand::new("status", "Show detailed session status").contextual(),
    ]);

    adapter
        .set_command_menu(&conversation, &detached)
        .await
        .unwrap();
    adapter
        .set_command_menu(&conversation, &attached)
        .await
        .unwrap();

    let requests = server.requests().await;
    let sent = requests
        .iter()
        .find(|request| request.method == "POST" && request.target.contains("/im/v1/messages?"))
        .unwrap();
    let sent_body: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
    let sent_card: serde_json::Value =
        serde_json::from_str(sent_body["content"].as_str().unwrap()).unwrap();
    assert_eq!(sent_card["header"]["title"]["content"], "Agentix commands");
    assert_eq!(
        sent_card["body"]["elements"][1]["behaviors"][0]["value"],
        serde_json::json!({"command": "/sessions"})
    );

    let updated = requests
        .iter()
        .find(|request| request.method == "PATCH")
        .unwrap();
    assert_eq!(updated.target, "/open-apis/im/v1/messages/om_mock_message");
    let updated_body: serde_json::Value = serde_json::from_str(&updated.body).unwrap();
    let updated_card: serde_json::Value =
        serde_json::from_str(updated_body["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        updated_card["body"]["elements"][2]["text"]["content"],
        "✌️ /status"
    );
    assert_eq!(
        updated_card["body"]["elements"][2]["behaviors"][0]["value"],
        serde_json::json!({"command": "/status"})
    );
}

#[tokio::test]
async fn feishu_mock_api_errors_are_channel_transport_errors() {
    let server = MockFeishuApi::start().await;
    server
        .fail_next("/open-apis/im/v1/messages", 230_001, "mock send failure")
        .await;
    let client = LarkClient::builder("mock-error-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);

    let error = adapter
        .send(
            &ConversationRef::new(ChannelKind::Feishu, "oc_mock_chat"),
            &OutboundView::text("Failure", "Expected"),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, ChannelError::Transport(message) if message.contains("mock send failure"))
    );
    let requests = server.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request
                .target
                .starts_with("/open-apis/auth/v3/tenant_access_token/internal"))
            .count(),
        1,
        "non-token errors must not refresh the tenant token"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.target.starts_with("/open-apis/im/v1/messages?"))
            .count(),
        1,
        "non-token errors must not retry the message"
    );
}

#[tokio::test]
async fn feishu_invalid_tenant_token_is_refreshed_and_the_request_is_retried_once() {
    let server = MockFeishuApi::start().await;
    server
        .fail_next(
            "/open-apis/im/v1/messages",
            99_991_663,
            "Invalid access token for authorization.",
        )
        .await;
    let client = LarkClient::builder("mock-refresh-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);

    let message = adapter
        .send(
            &ConversationRef::new(ChannelKind::Feishu, "oc_mock_chat"),
            &OutboundView::text("Working", "Working 0s"),
        )
        .await
        .unwrap();

    assert_eq!(message.message_id, "om_mock_message");
    let requests = server.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request
                .target
                .starts_with("/open-apis/auth/v3/tenant_access_token/internal"))
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.target.starts_with("/open-apis/im/v1/messages?"))
            .count(),
        2
    );
    let sends = requests
        .iter()
        .filter(|request| request.target.starts_with("/open-apis/im/v1/messages?"))
        .collect::<Vec<_>>();
    assert!(
        sends[0]
            .headers
            .contains("authorization: Bearer mock-tenant-token-1")
    );
    assert!(
        sends[1]
            .headers
            .contains("authorization: Bearer mock-tenant-token-2")
    );
    assert_eq!(server.successful_message_deliveries(), 1);
}

#[tokio::test]
async fn feishu_invalid_tenant_token_is_not_retried_more_than_once() {
    let server = MockFeishuApi::start().await;
    for _ in 0..2 {
        server
            .fail_next(
                "/open-apis/im/v1/messages",
                99_991_663,
                "Invalid access token for authorization.",
            )
            .await;
    }
    let client = LarkClient::builder("mock-refresh-limit-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);

    let error = adapter
        .send(
            &ConversationRef::new(ChannelKind::Feishu, "oc_mock_chat"),
            &OutboundView::text("Working", "Working 0s"),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ChannelError::Transport(message) if message.contains("99991663")));
    let requests = server.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request
                .target
                .starts_with("/open-apis/auth/v3/tenant_access_token/internal"))
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.target.starts_with("/open-apis/im/v1/messages?"))
            .count(),
        2
    );
    let sends = requests
        .iter()
        .filter(|request| request.target.starts_with("/open-apis/im/v1/messages?"))
        .collect::<Vec<_>>();
    assert!(
        sends[0]
            .headers
            .contains("authorization: Bearer mock-tenant-token-1")
    );
    assert!(
        sends[1]
            .headers
            .contains("authorization: Bearer mock-tenant-token-2")
    );
    assert_eq!(server.successful_message_deliveries(), 0);
}

#[tokio::test]
async fn feishu_failed_token_refresh_does_not_replay_the_message() {
    let server = MockFeishuApi::start().await;
    server
        .fail_next(
            "/open-apis/im/v1/messages",
            99_991_663,
            "Invalid access token for authorization.",
        )
        .await;
    server
        .fail_next(
            "/open-apis/auth/v3/tenant_access_token/internal",
            99_991_664,
            "mock token refresh failure",
        )
        .await;
    let client = LarkClient::builder("mock-refresh-failure-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);

    let error = adapter
        .send(
            &ConversationRef::new(ChannelKind::Feishu, "oc_mock_chat"),
            &OutboundView::text("Working", "Working 0s"),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, ChannelError::Transport(message) if message.contains("mock token refresh failure"))
    );
    let requests = server.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request
                .target
                .starts_with("/open-apis/auth/v3/tenant_access_token/internal"))
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.target.starts_with("/open-apis/im/v1/messages?"))
            .count(),
        1,
        "a failed refresh must not replay the message with no fresh token"
    );
    assert_eq!(server.successful_message_deliveries(), 0);
}

#[tokio::test]
async fn feishu_invalid_tenant_token_refreshes_all_outbound_mutations() {
    let server = MockFeishuApi::start().await;
    let client = LarkClient::builder("mock-mutations-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let conversation = ConversationRef::new(ChannelKind::Feishu, "oc_mock_chat");
    let actionable = OutboundView {
        title: "Approval".into(),
        subtitle: None,
        body: "Allow?".into(),
        status: ViewStatus::Waiting,
        actions: vec![ActionButton {
            label: "Allow".into(),
            token: "allow-token".into(),
            style: ActionStyle::Primary,
        }],
    };
    let message = adapter.send(&conversation, &actionable).await.unwrap();

    server
        .fail_next(
            "/open-apis/im/v1/messages/om_mock_message",
            99_991_663,
            "Invalid access token for authorization.",
        )
        .await;
    adapter.disable_actions(&message).await.unwrap();

    server
        .fail_next(
            "/open-apis/im/v1/messages/om_mock_message",
            99_991_663,
            "Invalid access token for authorization.",
        )
        .await;
    adapter
        .update(
            &conversation,
            &message,
            &OutboundView::text("Approval", "Selected: Allow"),
        )
        .await
        .unwrap();

    let detached = CommandMenu::new(vec![ChannelCommand::new(
        "sessions",
        "Browse running sessions",
    )]);
    server
        .fail_next(
            "/open-apis/im/v1/messages",
            99_991_663,
            "Invalid access token for authorization.",
        )
        .await;
    adapter
        .set_command_menu(&conversation, &detached)
        .await
        .unwrap();

    let attached = CommandMenu::new(vec![
        ChannelCommand::new("sessions", "Browse running sessions"),
        ChannelCommand::new("status", "Show detailed session status").contextual(),
    ]);
    server
        .fail_next(
            "/open-apis/im/v1/messages/om_mock_message",
            99_991_663,
            "Invalid access token for authorization.",
        )
        .await;
    adapter
        .set_command_menu(&conversation, &attached)
        .await
        .unwrap();

    let requests = server.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request
                .target
                .starts_with("/open-apis/auth/v3/tenant_access_token/internal"))
            .count(),
        5
    );
    let mutations = requests
        .iter()
        .filter(|request| request.target.starts_with("/open-apis/im/v1/messages"))
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 9);
    let expected_tokens = [1, 1, 2, 2, 3, 3, 4, 4, 5];
    for (request, token) in mutations.iter().zip(expected_tokens) {
        assert!(
            request
                .headers
                .contains(&format!("authorization: Bearer mock-tenant-token-{token}")),
            "unexpected authorization header for {} {}",
            request.method,
            request.target
        );
    }
}

fn message_event(timestamp: &str) -> serde_json::Value {
    message_event_from("om_inbound", timestamp, "ou_owner", "/sessions", "p2p")
}

fn message_event_from(
    message_id: &str,
    timestamp: &str,
    owner_open_id: &str,
    text: &str,
    chat_type: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "header": {
            "event_id": "ev_message",
            "event_type": "im.message.receive_v1",
            "app_id": "mock-app",
            "tenant_key": "tenant",
            "create_time": timestamp
        },
        "event": {
            "sender": {
                "sender_id": {"open_id": owner_open_id, "user_id": "owner_user"},
                "sender_type": "user",
                "tenant_key": "tenant"
            },
            "message": {
                "message_id": message_id,
                "chat_id": "oc_mock_chat",
                "chat_type": chat_type,
                "message_type": "text",
                "content": serde_json::json!({"text": text}).to_string(),
                "create_time": timestamp
            }
        }
    })
}

#[tokio::test]
async fn private_claim_persists_the_owner_and_authorizes_follow_up_messages() {
    let server = MockFeishuApi::start().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    server
        .push_event(message_event_from(
            "om_claim",
            &now,
            "ou_claimed",
            "/claim TEST-CODE",
            "p2p",
        ))
        .await;
    server
        .push_event(message_event_from(
            "om_second_claim",
            &now,
            "ou_second",
            "/claim TEST-CODE",
            "p2p",
        ))
        .await;
    server
        .push_event(message_event_from(
            "om_after_claim",
            &now,
            "ou_claimed",
            "/sessions",
            "p2p",
        ))
        .await;
    let client = LarkClient::builder("mock-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let claimer = Arc::new(RecordingOwnerClaimer::default());
    let adapter = FeishuAdapter::with_client(client, std::iter::empty::<String>())
        .with_owner_claimer(claimer.clone());
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(4);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(message.event_id, "om_after_claim");
    assert_eq!(message.owner_id, "ou_claimed");
    assert_eq!(
        message.payload,
        agentix_core::InboundPayload::Text("/sessions".into())
    );
    server.wait_for_acknowledgements(3).await;
    assert_eq!(
        claimer.claims.lock().unwrap().as_slice(),
        [("TEST-CODE".into(), "ou_claimed".into())]
    );

    let requests = server.requests().await;
    let confirmations = requests
        .iter()
        .filter(|request| request.target.starts_with("/open-apis/im/v1/messages?"))
        .collect::<Vec<_>>();
    assert_eq!(confirmations.len(), 1, "claim must be single-use");
    let body: serde_json::Value = serde_json::from_str(&confirmations[0].body).unwrap();
    assert_eq!(body["msg_type"], "text");
    assert!(body["content"].as_str().unwrap().contains("Owner claimed"));

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn feishu_claim_response_refreshes_an_invalid_tenant_token() {
    let server = MockFeishuApi::start().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    server
        .push_event(message_event_from(
            "om_claim_refresh",
            &now,
            "ou_claimed",
            "/claim TEST-CODE",
            "p2p",
        ))
        .await;
    server
        .fail_next(
            "/open-apis/im/v1/messages",
            99_991_663,
            "Invalid access token for authorization.",
        )
        .await;
    let client = LarkClient::builder("mock-claim-refresh-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let claimer = Arc::new(RecordingOwnerClaimer::default());
    let adapter = FeishuAdapter::with_client(client, std::iter::empty::<String>())
        .with_owner_claimer(claimer.clone());
    let shutdown = CancellationToken::new();
    let (sender, _receiver) = mpsc::channel(1);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    server.wait_for_acknowledgements(1).await;
    assert_eq!(
        claimer.claims.lock().unwrap().as_slice(),
        [("TEST-CODE".into(), "ou_claimed".into())]
    );
    let requests = server.requests().await;
    let confirmations = requests
        .iter()
        .filter(|request| request.target.starts_with("/open-apis/im/v1/messages?"))
        .collect::<Vec<_>>();
    assert_eq!(confirmations.len(), 2);
    assert_ne!(
        authorization_header(confirmations[0]),
        authorization_header(confirmations[1])
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

fn card_action_event() -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "header": {
            "event_id": "ev_action",
            "event_type": "card.action.trigger",
            "app_id": "mock-app",
            "tenant_key": "tenant",
            "create_time": "2"
        },
        "event": {
            "operator": {
                "open_id": "ou_owner",
                "user_id": "owner_user",
                "tenant_key": "tenant"
            },
            "context": {
                "open_message_id": "om_card",
                "open_chat_id": "oc_mock_chat"
            },
            "action": {
                "tag": "button",
                "name": "approve",
                "value": {"token": "opaque-action-token"}
            },
            "token": "verification-token",
            "host": "feishu",
            "delivery_type": "push"
        }
    })
}

fn card_command_event() -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "header": {
            "event_id": "ev_command",
            "event_type": "card.action.trigger",
            "app_id": "mock-app",
            "tenant_key": "tenant",
            "create_time": "3"
        },
        "event": {
            "operator": {
                "open_id": "ou_owner",
                "user_id": "owner_user",
                "tenant_key": "tenant"
            },
            "context": {
                "open_message_id": "om_menu",
                "open_chat_id": "oc_mock_chat"
            },
            "action": {
                "tag": "button",
                "name": "command",
                "value": {"command": "/status"}
            },
            "token": "verification-token",
            "host": "feishu",
            "delivery_type": "push"
        }
    })
}

#[tokio::test]
async fn feishu_reply_message_is_included_as_quoted_prompt_context() {
    let server = MockFeishuApi::start().await;
    server
        .set_message(
            "om_parent",
            serde_json::json!({
                "message_id": "om_parent",
                "msg_type": "text",
                "chat_id": "oc_mock_chat",
                "body": {"content": serde_json::json!({"text": "earlier line\nsecond line"}).to_string()}
            }),
        )
        .await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let mut event = message_event_from("om_reply", &now, "ou_owner", "continue with this", "p2p");
    event["event"]["message"]["parent_id"] = serde_json::json!("om_parent");
    server.push_event(event).await;
    let client = LarkClient::builder("mock-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(1);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        message.payload,
        agentix_core::InboundPayload::Text(
            "**Quoted message**\n\n> earlier line\n> second line\n\ncontinue with this".into()
        )
    );
    assert!(server.requests().await.iter().any(|request| {
        request.method == "GET"
            && request
                .target
                .starts_with("/open-apis/im/v1/messages/om_parent")
    }));

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn feishu_reply_lookup_refreshes_an_invalid_tenant_token() {
    let server = MockFeishuApi::start().await;
    server
        .set_message(
            "om_parent_refresh",
            serde_json::json!({
                "message_id": "om_parent_refresh",
                "msg_type": "text",
                "chat_id": "oc_mock_chat",
                "body": {"content": serde_json::json!({"text": "fresh context"}).to_string()}
            }),
        )
        .await;
    server
        .fail_next(
            "/open-apis/im/v1/messages/om_parent_refresh",
            99_991_663,
            "Invalid access token for authorization.",
        )
        .await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let mut event = message_event_from("om_reply_refresh", &now, "ou_owner", "continue", "p2p");
    event["event"]["message"]["parent_id"] = serde_json::json!("om_parent_refresh");
    server.push_event(event).await;
    let client = LarkClient::builder("mock-reply-refresh-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(1);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        message.payload,
        agentix_core::InboundPayload::Text(
            "**Quoted message**\n\n> fresh context\n\ncontinue".into()
        )
    );
    let requests = server.requests().await;
    let lookups = requests
        .iter()
        .filter(|request| {
            request
                .target
                .starts_with("/open-apis/im/v1/messages/om_parent_refresh")
        })
        .collect::<Vec<_>>();
    assert_eq!(lookups.len(), 2);
    assert_ne!(
        authorization_header(lookups[0]),
        authorization_header(lookups[1])
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn feishu_reply_lookup_failure_keeps_the_new_prompt() {
    let server = MockFeishuApi::start().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let mut event = message_event_from("om_reply", &now, "ou_owner", "keep going", "p2p");
    event["event"]["message"]["parent_id"] = serde_json::json!("om_missing");
    server.push_event(event).await;
    let client = LarkClient::builder("mock-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(1);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        message.payload,
        agentix_core::InboundPayload::Text("keep going".into())
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn feishu_reply_to_card_quotes_visible_text_without_button_labels() {
    let server = MockFeishuApi::start().await;
    server
        .set_message(
            "om_parent_card",
            serde_json::json!({
                "message_id": "om_parent_card",
                "msg_type": "interactive",
                "chat_id": "oc_mock_chat",
                "body": {"content": serde_json::json!({
                    "title": "Codex · session",
                    "elements": [
                        [{"tag": "text", "text": "Agent output"}],
                        [{"tag": "button", "text": "Stop", "type": "danger"}]
                    ]
                }).to_string()}
            }),
        )
        .await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let mut event = message_event_from("om_reply", &now, "ou_owner", "continue", "p2p");
    event["event"]["message"]["parent_id"] = serde_json::json!("om_parent_card");
    server.push_event(event).await;
    let client = LarkClient::builder("mock-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(1);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        message.payload,
        agentix_core::InboundPayload::Text(
            "**Quoted message**\n\n> Codex · session\n> Agent output\n\ncontinue".into()
        )
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn feishu_slash_commands_ignore_reply_context() {
    let server = MockFeishuApi::start().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let mut event = message_event_from("om_reply", &now, "ou_owner", "/status", "p2p");
    event["event"]["message"]["parent_id"] = serde_json::json!("om_parent");
    server.push_event(event).await;
    let client = LarkClient::builder("mock-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(1);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        message.payload,
        agentix_core::InboundPayload::Text("/status".into())
    );
    assert!(!server.requests().await.iter().any(|request| {
        request.method == "GET" && request.target.contains("/im/v1/messages/om_parent")
    }));

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn feishu_mock_long_connection_forwards_messages_and_card_actions() {
    let server = MockFeishuApi::start().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    server.push_event(message_event(&now)).await;
    server.push_event(card_action_event()).await;
    server.push_event(card_command_event()).await;
    let client = LarkClient::builder("mock-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let adapter = FeishuAdapter::with_client(client, ["ou_owner"]);
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(4);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(message.event_id, "om_inbound");
    assert_eq!(message.owner_id, "ou_owner");
    assert_eq!(
        message.payload,
        agentix_core::InboundPayload::Text("/sessions".into())
    );
    let action = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(action.event_id, "card:om_card:opaque-action-token");
    assert_eq!(
        action.payload,
        agentix_core::InboundPayload::Action {
            token: "opaque-action-token".into(),
            message: Some(MessageRef::new(
                ConversationRef::new(ChannelKind::Feishu, "oc_mock_chat"),
                "om_card"
            ))
        }
    );
    let command = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(command.event_id, "card:om_menu:/status");
    assert_eq!(
        command.payload,
        agentix_core::InboundPayload::Text("/status".into())
    );
    server.wait_for_acknowledgements(3).await;

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let targets = server
        .requests()
        .await
        .into_iter()
        .map(|request| request.target)
        .collect::<Vec<_>>();
    assert!(
        targets
            .iter()
            .any(|target| target == "/callback/ws/endpoint")
    );
    assert!(
        targets
            .iter()
            .any(|target| target.starts_with("/open-apis/bot/v3/info"))
    );
}

#[test]
fn background_turn_cards_use_purple_quoted_content_and_keep_actions() {
    let view = OutboundView {
        title: "Codex · Background task".into(),
        subtitle: Some("Background turn 12345678 · Completed".into()),
        body: "**👤 You**\n\n> Run checks\n\n**🤖 Codex**\n\n> All checks passed".into(),
        status: serde_json::from_str("\"background\"").unwrap(),
        actions: vec![ActionButton {
            label: "Attach".into(),
            token: "attach-token".into(),
            style: ActionStyle::Primary,
        }],
    };
    let value = render_card(&view).unwrap().card().to_json().into_value();
    assert_eq!(value["header"]["template"], "purple");
    let quote = &value["body"]["elements"][0]["columns"][0];
    assert_eq!(quote["background_style"], "rgba(128,64,192,0.12)");
    assert!(
        quote["elements"][0]["content"]
            .as_str()
            .unwrap()
            .contains("> All checks passed")
    );
    assert_eq!(value["body"]["elements"][1]["text"]["content"], "Attach");
}
