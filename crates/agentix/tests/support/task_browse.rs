use super::*;
use serde_json::{Value, json};

// Exercise identical navigation through each actual channel's inbound transport and renderer.
enum Browser {
    Telegram(MockTelegramApi),
    Feishu(MockFeishuApi),
}

impl Browser {
    async fn requests(&self) -> Vec<String> {
        match self {
            Self::Telegram(api) => api.requests().await.into_iter().map(|r| r.body).collect(),
            Self::Feishu(api) => api.requests().await.into_iter().map(|r| r.body).collect(),
        }
    }

    async fn command(&self, sequence: u32, text: &str) {
        match self {
            Self::Telegram(api) => {
                api.push_updates(vec![telegram_message(
                    u64::from(sequence),
                    i32::try_from(sequence).unwrap(),
                    text,
                )])
                .await;
            }
            Self::Feishu(api) => {
                api.push_event(feishu_message(
                    &format!("browse_{sequence}"),
                    &timestamp(),
                    text,
                ))
                .await;
            }
        }
    }

    async fn click(&self, sequence: u32, token: &str) {
        match self {
            Self::Telegram(api) => {
                api.push_updates(vec![json!({"update_id":sequence,"callback_query":{
                    "id":format!("browse_{sequence}"),
                    "from":{"id":42,"is_bot":false,"first_name":"Owner"},
                    "chat_instance":"mock-chat",
                    "message":{"message_id":77,"date":1,"chat":{"id":42,"type":"private"},"text":"Board"},
                    "data":token
                }})]).await;
            }
            Self::Feishu(api) => {
                api.push_event(json!({"schema":"2.0","header":{
                    "event_id":format!("browse_{sequence}"),"event_type":"card.action.trigger",
                    "app_id":"mock-app","tenant_key":"tenant","create_time":timestamp()
                },"event":{
                    "operator":{"open_id":"ou_owner","user_id":"owner_user","tenant_key":"tenant"},
                    "context":{"open_message_id":"om_mock_message","open_chat_id":"oc_mock_chat"},
                    "action":{"tag":"button","name":"browse","value":{"token":token}},
                    "token":"verification-token","host":"feishu","delivery_type":"push"
                }}))
                .await;
            }
        }
    }

    async fn view(&self, after: usize, label: &str) -> Value {
        let body = wait_for_value(|| async {
            self.requests().await.into_iter().skip(after).find(|body| {
                serde_json::from_str(body)
                    .ok()
                    .and_then(|v| button_token(&v, label))
                    .is_some()
            })
        })
        .await;
        serde_json::from_str(&body).unwrap()
    }

    async fn follow(&self, sequence: u32, view: &Value, label: &str, next_label: &str) -> Value {
        let after = self.requests().await.len();
        self.click(sequence, &button_token(view, label).unwrap())
            .await;
        self.view(after, next_label).await
    }

    fn assert_markdown(&self, view: &Value, bold: &str) {
        let body = markdown_content(view).expect("channel must send a Markdown element");
        match self {
            Self::Telegram(_) => {
                assert!(body.contains(&format!("*{bold}*")), "{body}");
                assert!(
                    !body.contains(&format!("**{bold}**")),
                    "MarkdownV2 bold conversion"
                );
                assert!(body.len() < 4096);
            }
            Self::Feishu(_) => assert!(body.contains(&format!("**{bold}**")), "{body}"),
        }
        assert!(!body.contains("taskcli:"));
        assert!(!body.contains("```mermaid"));
    }
}

fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string()
}

fn find_nested(value: &Value, predicate: &impl Fn(&Value) -> Option<String>) -> Option<String> {
    predicate(value).or_else(|| match value {
        Value::Object(values) => values.values().find_map(|v| find_nested(v, predicate)),
        Value::Array(values) => values.iter().find_map(|v| find_nested(v, predicate)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
            .and_then(|v| find_nested(&v, predicate)),
        _ => None,
    })
}

fn button_token(value: &Value, label: &str) -> Option<String> {
    find_nested(value, &|v| {
        if v["text"] == label {
            v["callback_data"].as_str().map(str::to_owned)
        } else if v["text"]["content"] == label {
            v["behaviors"][0]["value"]["token"]
                .as_str()
                .map(str::to_owned)
        } else {
            None
        }
    })
}

fn markdown_content(value: &Value) -> Option<String> {
    find_nested(value, &|v| {
        if v["parse_mode"] == "MarkdownV2" {
            v["text"].as_str().map(str::to_owned)
        } else if v["tag"] == "markdown" {
            v["content"].as_str().map(str::to_owned)
        } else {
            None
        }
    })
}

async fn browse_round_trip(telegram: bool) {
    let (_dir, service, id) = task_board_fixture().await;
    let state = service.store().snapshot().await.unwrap();
    service.execute(json!({"command":"plan.revise","task":id,"body":"## Plan\n\n**Bold plan** and `code`\n\n- Test navigation"}), agentix_task::WriteOptions {
        session_ref:Some("thr_tasks".into()), lease_token:Some(state.leases[0].token.clone()),
        ..agentix_task::WriteOptions::default()
    }).await.unwrap();
    let path = service
        .config()
        .output_dir()
        .join(&state.jobs[0].document_path);
    let document = std::fs::read_to_string(&path)
        .unwrap()
        .replace(
            "<!-- taskcli:goal:start -->",
            "<!-- taskcli:goal:start -->\n**Bold goal**",
        )
        .replace(
            "<!-- taskcli:notes:start -->",
            "<!-- taskcli:notes:start -->\n**Bold notes**",
        );
    std::fs::write(path, document).unwrap();
    let before = service.store().snapshot().await.unwrap();
    let codex = MockCodexAppServer::start();
    codex
        .add_thread(MockThread::new(
            "thr_tasks",
            "Task integration",
            "/work/tasks",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(codex.endpoint()).await.unwrap());
    let shutdown = CancellationToken::new();
    let (browser, stack) = if telegram {
        let api = MockTelegramApi::start().await;
        let channel = Arc::new(TelegramAdapter::with_bot(
            Bot::new("test-token").set_api_url(api.api_url().parse().unwrap()),
            TelegramPolicy::new([42]),
        ));
        let stack =
            run_stack_with_tasks(client, channel, shutdown.clone(), Some(service.clone())).await;
        (Browser::Telegram(api), stack)
    } else {
        let api = MockFeishuApi::start().await;
        let lark = LarkClient::builder("mock-app", "mock-secret")
            .base_url(api.base_url())
            .max_retries(1)
            .build()
            .unwrap();
        let stack = run_stack_with_tasks(
            client,
            Arc::new(FeishuAdapter::with_client(lark, ["ou_owner"])),
            shutdown.clone(),
            Some(service.clone()),
        )
        .await;
        (Browser::Feishu(api), stack)
    };
    // Global navigation must work before attachment.
    browser.command(400, "/dashboard").await;
    let dashboard = browser.view(0, "Channel tests").await;
    let board = browser
        .follow(401, &dashboard, "Channel tests", "Channel task")
        .await;
    let task = browser.follow(402, &board, "Channel task", "Job").await;
    browser.assert_markdown(&task, "Bold plan");
    assert!(markdown_content(&task).unwrap().contains("`code`"));
    let job = browser.follow(403, &task, "Job", "Channel task").await;
    browser.assert_markdown(&job, "Bold goal");
    browser.assert_markdown(&job, "Bold notes");
    let task_again = browser.follow(404, &job, "Channel task", "Job").await;
    browser.assert_markdown(&task_again, "Bold plan");
    let job_again = browser
        .follow(405, &task_again, "Job", "Project board")
        .await;
    let board_again = browser
        .follow(406, &job_again, "Project board", "Dashboard")
        .await;
    browser
        .follow(407, &board_again, "Dashboard", "Channel tests")
        .await;

    browser.command(408, "/attach thr_tasks").await;
    let after = browser.requests().await.len();
    browser.command(409, "/board").await;
    let board = browser.view(after, "Channel task").await;
    assert!(markdown_content(&board).unwrap().contains("Current"));
    let after = browser.requests().await.len();
    browser.command(410, "/jobs").await;
    let jobs = browser.view(after, "IM integration").await;
    let job = browser
        .follow(411, &jobs, "IM integration", "Channel task")
        .await;
    browser.assert_markdown(&job, "Bold notes");
    assert_eq!(
        service.store().snapshot().await.unwrap(),
        before,
        "transport browsing must be read-only"
    );
    shutdown.cancel();
    join_stack(stack).await;
}

#[tokio::test]
async fn telegram_dashboard_session_board_jobs_and_markdown_navigation_round_trip() {
    browse_round_trip(true).await;
}

#[tokio::test]
async fn feishu_dashboard_session_board_jobs_and_markdown_navigation_round_trip() {
    browse_round_trip(false).await;
}
