use serde_json::Value;
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
};

#[derive(Debug)]
pub struct Request {
    pub path: String,
    pub authorization: String,
    pub body: Value,
}
pub struct Server {
    pub url: String,
    pub requests: mpsc::UnboundedReceiver<Request>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    pub async fn new(
        handler: impl Fn(&Request) -> (u16, Vec<(String, String)>, Value) + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/", listener.local_addr().unwrap());
        let (tx, requests) = mpsc::unbounded_channel();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let handler = handler.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buffer = [0; 4096];
                    let end = loop {
                        let n = stream.read(&mut buffer).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            break end + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&bytes[..end]).into_owned();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while bytes.len() < end + length {
                        let n = stream.read(&mut buffer).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                    let request = Request {
                        path: headers
                            .lines()
                            .next()
                            .unwrap()
                            .split_whitespace()
                            .nth(1)
                            .unwrap()
                            .into(),
                        authorization: headers
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                                    .map(|(_, value)| value.trim().to_owned())
                            })
                            .unwrap_or_default(),
                        body: serde_json::from_slice(&bytes[end..end + length])
                            .unwrap_or(Value::Null),
                    };
                    let (status, extra, body) = handler(&request);
                    tx.send(request).unwrap();
                    let body = body.to_string();
                    let mut headers = String::new();
                    for (key, value) in extra {
                        use std::fmt::Write;
                        write!(headers, "{key}: {value}\r\n").unwrap();
                    }
                    let response = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
}
