use crate::api::Api;
use agentix_domain::ChannelError;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::{client::generate_key, derive_accept_key},
        protocol::Role,
    },
};

async fn connect(api: &Api, url: &str) -> Result<WebSocketStream<reqwest::Upgraded>, ChannelError> {
    let mut url = url::Url::parse(url)
        .map_err(|_| ChannelError::InvalidPayload("invalid Slack socket URL".into()))?;
    let scheme = match url.scheme() {
        "wss" => "https",
        "ws" if url
            .host_str()
            .is_some_and(|host| host == "127.0.0.1" || host == "localhost" || host == "[::1]") =>
        {
            "http"
        }
        _ => {
            return Err(ChannelError::InvalidPayload(
                "Slack socket requires wss".into(),
            ));
        }
    };
    url.set_scheme(scheme)
        .map_err(|()| ChannelError::InvalidPayload("invalid Slack socket scheme".into()))?;
    let key = generate_key();
    // Reuse reqwest's TLS and proxy support for the HTTP upgrade, including SOCKS.
    let response = api
        .client
        .get(url)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", &key)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| ChannelError::Transport(error.without_url().to_string()))?;
    if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS
        || response
            .headers()
            .get("sec-websocket-accept")
            .and_then(|value| value.to_str().ok())
            != Some(derive_accept_key(key.as_bytes()).as_str())
    {
        return Err(ChannelError::Transport(
            "Slack WebSocket handshake rejected".into(),
        ));
    }
    let stream = response
        .upgrade()
        .await
        .map_err(|_| ChannelError::Transport("Slack WebSocket upgrade failed".into()))?;
    Ok(WebSocketStream::from_raw_socket(
        stream,
        Role::Client,
        Some(
            tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                .max_message_size(Some(1024 * 1024)),
        ),
    )
    .await)
}

pub(crate) async fn listen(api: &Api, events: mpsc::Sender<Value>) -> Result<(), ChannelError> {
    let mut backoff = 1;
    loop {
        let result = connection(api, &events).await;
        match result {
            Ok(()) => backoff = 1,
            Err(ChannelError::Rejected(error)) => return Err(ChannelError::Rejected(error)),
            Err(error) => {
                tracing::warn!(%error,"Slack socket reconnecting");
                backoff = (backoff * 2).min(30);
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
    }
}

async fn connection(api: &Api, events: &mpsc::Sender<Value>) -> Result<(), ChannelError> {
    let response = api.call("apps.connections.open", &json!({})).await?;
    let url = response["url"]
        .as_str()
        .ok_or_else(|| ChannelError::InvalidPayload("missing Slack socket URL".into()))?;
    let mut socket = connect(api, url).await?;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(90), socket.next())
            .await
            .map_err(|_| ChannelError::Transport("Slack socket heartbeat timed out".into()))?;
        match frame {
            Some(Ok(Message::Text(text))) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if value["type"] == "hello" {
                    tracing::info!(
                        app_id = value["connection_info"]["app_id"]
                            .as_str()
                            .unwrap_or("unknown"),
                        connections = value["num_connections"].as_u64(),
                        "Slack Socket Mode connected"
                    );
                }
                if value["type"] == "disconnect" {
                    return Ok(());
                }
                if let Some(id) = value["envelope_id"].as_str() {
                    let started = std::time::Instant::now();
                    let slash = value["type"] == "slash_commands";
                    if slash {
                        tracing::info!(
                            envelope_id = id,
                            command = value["payload"]["command"].as_str().unwrap_or("unknown"),
                            "Slack slash command received"
                        );
                    }
                    let ack = json!({"envelope_id":id}).to_string();
                    // Do not ACK data we cannot retain. Slack retries after reconnect.
                    events.try_send(value).map_err(|_| {
                        ChannelError::Transport("Slack inbound queue is full or closed".into())
                    })?;
                    socket
                        .send(Message::Text(ack.into()))
                        .await
                        .map_err(|_| ChannelError::Transport("Slack ACK failed".into()))?;
                    if slash {
                        tracing::info!(
                            elapsed_ms = started.elapsed().as_millis(),
                            "Slack slash command acknowledged"
                        );
                    }
                }
            }
            Some(Ok(Message::Ping(data))) => socket
                .send(Message::Pong(data))
                .await
                .map_err(|_| ChannelError::Transport("Slack pong failed".into()))?,
            Some(Ok(Message::Close(_))) | None => return Ok(()),
            Some(Err(_)) => {
                return Err(ChannelError::Transport("Slack socket disconnected".into()));
            }
            _ => {}
        }
    }
}
