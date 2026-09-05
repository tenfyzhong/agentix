use std::collections::VecDeque;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub target: String,
    pub body: String,
}

#[derive(Debug)]
struct PlannedFailure {
    method: String,
    description: String,
}

pub struct MockTelegramApi {
    api_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    updates: Arc<Mutex<VecDeque<Vec<Value>>>>,
    failures: Arc<Mutex<VecDeque<PlannedFailure>>>,
    task: JoinHandle<()>,
}

impl MockTelegramApi {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let updates = Arc::new(Mutex::new(VecDeque::new()));
        let failures = Arc::new(Mutex::new(VecDeque::new()));
        let captured = Arc::clone(&requests);
        let pending_updates = Arc::clone(&updates);
        let planned_failures = Arc::clone(&failures);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let requests = Arc::clone(&captured);
                let updates = Arc::clone(&pending_updates);
                let failures = Arc::clone(&planned_failures);
                tokio::spawn(async move {
                    serve_request(stream, requests, updates, failures).await;
                });
            }
        });
        Self {
            api_url: format!("http://{address}/"),
            requests,
            updates,
            failures,
            task,
        }
    }

    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    pub async fn push_updates(&self, updates: Vec<Value>) {
        self.updates.lock().await.push_back(updates);
    }

    pub async fn fail_next(&self, method: &str, description: &str) {
        self.failures.lock().await.push_back(PlannedFailure {
            method: method.to_ascii_lowercase(),
            description: description.into(),
        });
    }

    pub async fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().await.clone()
    }
}

impl Drop for MockTelegramApi {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_request(
    mut stream: TcpStream,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    updates: Arc<Mutex<VecDeque<Vec<Value>>>>,
    failures: Arc<Mutex<VecDeque<PlannedFailure>>>,
) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    let api_method = request
        .target
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    requests.lock().await.push(request);

    let failure = {
        let mut failures = failures.lock().await;
        failures
            .front()
            .is_some_and(|failure| failure.method == api_method)
            .then(|| failures.pop_front().unwrap())
    };
    let body = if let Some(failure) = failure {
        json!({
            "ok": false,
            "error_code": 400,
            "description": failure.description
        })
    } else {
        match api_method.as_str() {
            "getme" => json!({
                "ok": true,
                "result": {
                    "id": 9001,
                    "is_bot": true,
                    "first_name": "Agentix",
                    "username": "agentix_bot",
                    "can_join_groups": true,
                    "can_read_all_group_messages": false,
                    "supports_inline_queries": false,
                    "has_main_web_app": false
                }
            }),
            "getupdates" => {
                let batch = updates.lock().await.pop_front().unwrap_or_default();
                if batch.is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                json!({"ok": true, "result": batch})
            }
            "sendmessage" | "editmessagetext" | "editmessagereplymarkup" => json!({
                "ok": true,
                "result": {
                    "message_id": 77,
                    "date": 1,
                    "chat": {"id": 42, "type": "private", "first_name": "Owner"},
                    "text": "mock response"
                }
            }),
            _ => json!({"ok": true, "result": true}),
        }
    };
    write_response(&mut stream, &body.to_string()).await;
}

async fn read_request(stream: &mut TcpStream) -> Option<CapturedRequest> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let (header_end, content_length) = loop {
        let read = stream.read(&mut buffer).await.unwrap();
        if read == 0 {
            return None;
        }
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
        if read == 0 {
            return None;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
    let mut request_line = headers.lines().next().unwrap().split_whitespace();
    Some(CapturedRequest {
        method: request_line.next().unwrap().into(),
        target: request_line.next().unwrap().into(),
        body: String::from_utf8(request[header_end..header_end + content_length].to_vec()).unwrap(),
    })
}

async fn write_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}
