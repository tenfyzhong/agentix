use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::{SinkExt, StreamExt};
use larksuite_oapi_sdk_rs::ws::proto::{Frame, Header};
use prost::Message as _;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub target: String,
    pub headers: String,
    pub body: String,
}

#[derive(Debug)]
struct PlannedFailure {
    target: String,
    code: i64,
    message: String,
    http_status: u16,
}

pub struct MockFeishuApi {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    failures: Arc<Mutex<VecDeque<PlannedFailure>>>,
    messages: Arc<Mutex<HashMap<String, Value>>>,
    events: Arc<Mutex<VecDeque<Value>>>,
    acknowledgements: Arc<AtomicUsize>,
    successful_message_deliveries: Arc<AtomicUsize>,
    http_task: JoinHandle<()>,
    websocket_task: JoinHandle<()>,
}

impl MockFeishuApi {
    pub async fn start() -> Self {
        let websocket_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let websocket_address = websocket_listener.local_addr().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let failures = Arc::new(Mutex::new(VecDeque::new()));
        let messages = Arc::new(Mutex::new(HashMap::new()));
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let acknowledgements = Arc::new(AtomicUsize::new(0));
        let tenant_token_issuances = Arc::new(AtomicUsize::new(0));
        let successful_message_deliveries = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&requests);
        let planned = Arc::clone(&failures);
        let stored_messages = Arc::clone(&messages);
        let token_issuances = Arc::clone(&tenant_token_issuances);
        let message_deliveries = Arc::clone(&successful_message_deliveries);
        let websocket_url =
            format!("ws://{websocket_address}/ws?service_id=42&device_id=mock-device");
        let bootstrap_url = websocket_url.clone();
        let http_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let requests = Arc::clone(&captured);
                let failures = Arc::clone(&planned);
                let messages = Arc::clone(&stored_messages);
                let tenant_token_issuances = Arc::clone(&token_issuances);
                let successful_message_deliveries = Arc::clone(&message_deliveries);
                let websocket_url = bootstrap_url.clone();
                tokio::spawn(async move {
                    serve_request(
                        stream,
                        requests,
                        failures,
                        messages,
                        tenant_token_issuances,
                        successful_message_deliveries,
                        &websocket_url,
                    )
                    .await;
                });
            }
        });
        let pending_events = Arc::clone(&events);
        let received_acks = Arc::clone(&acknowledgements);
        let websocket_task = tokio::spawn(async move {
            while let Ok((stream, _)) = websocket_listener.accept().await {
                let events = Arc::clone(&pending_events);
                let acknowledgements = Arc::clone(&received_acks);
                tokio::spawn(async move {
                    serve_websocket(stream, events, acknowledgements).await;
                });
            }
        });
        Self {
            base_url: format!("http://{address}"),
            requests,
            failures,
            messages,
            events,
            acknowledgements,
            successful_message_deliveries,
            http_task,
            websocket_task,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn fail_next(&self, target: &str, code: i64, message: &str) {
        self.failures.lock().await.push_back(PlannedFailure {
            target: target.into(),
            code,
            message: message.into(),
            http_status: 200,
        });
    }

    pub async fn rate_limit_next(&self, target: &str) {
        self.failures.lock().await.push_back(PlannedFailure {
            target: target.into(),
            code: 99_991_400,
            message: "Too Many Requests".into(),
            http_status: 429,
        });
    }

    pub async fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().await.clone()
    }

    pub fn successful_message_deliveries(&self) -> usize {
        self.successful_message_deliveries.load(Ordering::Acquire)
    }

    pub async fn set_message(&self, message_id: &str, message: Value) {
        self.messages
            .lock()
            .await
            .insert(message_id.to_owned(), message);
    }

    pub async fn push_event(&self, event: Value) {
        self.events.lock().await.push_back(event);
    }

    pub async fn wait_for_acknowledgements(&self, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while self.acknowledgements.load(Ordering::Acquire) < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

impl Drop for MockFeishuApi {
    fn drop(&mut self) {
        self.http_task.abort();
        self.websocket_task.abort();
    }
}

async fn serve_request(
    mut stream: TcpStream,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    failures: Arc<Mutex<VecDeque<PlannedFailure>>>,
    messages: Arc<Mutex<HashMap<String, Value>>>,
    tenant_token_issuances: Arc<AtomicUsize>,
    successful_message_deliveries: Arc<AtomicUsize>,
    websocket_url: &str,
) {
    let request = read_request(&mut stream).await;
    let target = request.target.clone();
    let method = request.method.clone();
    requests.lock().await.push(request);

    let failure = {
        let mut failures = failures.lock().await;
        failures
            .front()
            .is_some_and(|failure| target.contains(&failure.target))
            .then(|| failures.pop_front().unwrap())
    };
    let status = failure.as_ref().map_or(200, |failure| failure.http_status);
    let body = if let Some(failure) = failure {
        serde_json::json!({"code": failure.code, "msg": failure.message}).to_string()
    } else if target.starts_with("/callback/ws/endpoint") {
        serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": {
                "URL": websocket_url,
                "ClientConfig": {
                    "PingInterval": 120,
                    "ReconnectCount": 0,
                    "ReconnectInterval": 1,
                    "ReconnectNonce": 0
                }
            }
        })
        .to_string()
    } else if target.starts_with("/open-apis/auth/v3/tenant_access_token/internal") {
        let issuance = tenant_token_issuances.fetch_add(1, Ordering::AcqRel) + 1;
        serde_json::json!({
            "code": 0,
            "msg": "ok",
            "tenant_access_token": format!("mock-tenant-token-{issuance}"),
            "expire": 3600
        })
        .to_string()
    } else if target.starts_with("/open-apis/bot/v3/info") {
        serde_json::json!({
            "code": 0,
            "msg": "ok",
            "bot": {
                "app_id": "mock-app",
                "open_id": "ou_mock_bot",
                "user_id": "mock_bot_user",
                "app_name": "Agentix"
            }
        })
        .to_string()
    } else if method == "GET"
        && let Some(message_id) = request_message_id(&target)
    {
        let message = messages.lock().await.get(message_id).cloned();
        match message {
            Some(message) => serde_json::json!({
                "code": 0,
                "msg": "ok",
                "data": {"items": [message]}
            })
            .to_string(),
            None => serde_json::json!({"code": 230_001, "msg": "unknown mock message"}).to_string(),
        }
    } else if target.starts_with("/open-apis/im/v1/messages") {
        if method == "POST" && target.contains("?receive_id_type=") {
            successful_message_deliveries.fetch_add(1, Ordering::AcqRel);
        }
        serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": {"message_id": "om_mock_message", "chat_id": "oc_mock_chat"}
        })
        .to_string()
    } else {
        serde_json::json!({"code": 404, "msg": format!("unhandled mock target: {target}")})
            .to_string()
    };
    write_response(&mut stream, &body, status).await;
}

fn request_message_id(target: &str) -> Option<&str> {
    let path = target.split('?').next()?;
    path.strip_prefix("/open-apis/im/v1/messages/")
        .filter(|message_id| !message_id.is_empty())
}

async fn serve_websocket(
    stream: TcpStream,
    events: Arc<Mutex<VecDeque<Value>>>,
    acknowledgements: Arc<AtomicUsize>,
) {
    let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
    loop {
        let event = loop {
            if let Some(event) = events.lock().await.pop_front() {
                break event;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let frame = Frame {
            seq_id: acknowledgements.load(Ordering::Acquire) as u64 + 1,
            log_id: 100,
            service: 42,
            method: 1,
            headers: vec![
                Header {
                    key: "type".into(),
                    value: "event".into(),
                },
                Header {
                    key: "sum".into(),
                    value: "1".into(),
                },
            ],
            payload_encoding: Some("json".into()),
            payload_type: Some("event".into()),
            payload: Some(serde_json::to_vec(&event).unwrap()),
            log_id_new: None,
        };
        if websocket
            .send(Message::Binary(frame.encode_to_vec().into()))
            .await
            .is_err()
        {
            return;
        }
        match websocket.next().await {
            Some(Ok(Message::Binary(payload))) if Frame::decode(payload.as_ref()).is_ok() => {
                acknowledgements.fetch_add(1, Ordering::Release);
            }
            _ => return,
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let (header_end, content_length) = loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert_ne!(read, 0, "request ended before its headers were complete");
        request.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::parse::<usize>)
                })
                .transpose()
                .unwrap()
                .unwrap_or_default();
            break (header_end + 4, content_length);
        }
    };
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await.unwrap();
        assert_ne!(read, 0, "request ended before its body was complete");
        request.extend_from_slice(&buffer[..read]);
    }
    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
    let mut request_line = headers.lines().next().unwrap().split_whitespace();
    CapturedRequest {
        method: request_line.next().unwrap().into(),
        target: request_line.next().unwrap().into(),
        headers: headers.into(),
        body: String::from_utf8(request[header_end..header_end + content_length].to_vec()).unwrap(),
    }
}

async fn write_response(stream: &mut TcpStream, body: &str, status: u16) {
    let response = format!(
        "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}
