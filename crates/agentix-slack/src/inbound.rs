//! Inbound delivery and owner bootstrap. Socket ACKs run independently.
use crate::{SlackAdapter, events, normalize_event, socket};
use agentix_domain::{ChannelAdapter, ChannelError, InboundEnvelope, OutboundView};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

impl SlackAdapter {
    async fn handle_claim(&self, event: &Value, team: &str, bot: &str) -> bool {
        let payload = &event["payload"];
        let slash = event["type"] == "slash_commands"
            && matches!(payload["command"].as_str(), Some("/agentix" | "/claim"));
        let message = if slash { payload } else { &payload["event"] };
        let Some(text) = message["text"].as_str() else {
            return false;
        };
        let mut words = text.split_whitespace();
        if !(slash && payload["command"] == "/claim") && words.next() != Some("/claim") {
            return false;
        }
        let Some(code) = words.next().filter(|_| words.next().is_none()) else {
            return true;
        };
        let Some(owner) = message[if slash { "user_id" } else { "user" }].as_str() else {
            return true;
        };
        let Some(channel) = message[if slash { "channel_id" } else { "channel" }].as_str() else {
            return true;
        };
        if (!slash && (event["type"] != "events_api" || message["type"] != "message"))
            || payload["team_id"] != team
            || message.get("subtype").is_some()
            || message.get("bot_id").is_some()
            || owner == bot
            || !channel.starts_with('D')
        {
            return true;
        }
        let Some(claimer) = &self.owner_claimer else {
            return true;
        };
        let mut owners = self.owners.lock().await;
        if !owners.is_empty() {
            return true;
        }
        let result = claimer.claim(code, owner).await;
        if matches!(result, Ok(true)) {
            owners.push(owner.into());
        }
        drop(owners);
        let body = match result {
            Ok(true) => match self.sync_commands(team, true).await {
                Ok(()) => {
                    "Owner linked. Choose the sessions command from this app’s command menu to begin."
                }
                Err(error) => {
                    tracing::warn!(%error, "Slack command sync after owner claim failed");
                    "Owner linked, but the command menu could not be updated. Check the server logs and restart Agentix to retry."
                }
            },
            Ok(false) => "Invalid or expired claim code.",
            Err(_) => "Owner could not be saved. Check the server logs and try again.",
        };
        if let Err(error) = self
            .send(
                &events::conversation(team, channel, None),
                &OutboundView::text("Owner claim", body),
            )
            .await
        {
            tracing::warn!(%error,"Slack claim reply failed");
        }
        true
    }
    async fn sync_commands(&self, team: &str, has_owner: bool) -> Result<(), ChannelError> {
        if let Some(sync) = &self.command_sync {
            let changed = sync.sync_for_owner(team, has_owner).await?;
            tracing::info!(changed, has_owner, "Slack slash commands synchronized");
        } else {
            tracing::warn!("Slack slash command sync disabled: configure channel.slack.app_id");
        }
        Ok(())
    }

    pub(crate) async fn run_inbound(
        &self,
        inbound: mpsc::Sender<InboundEnvelope>,
        shutdown: CancellationToken,
    ) -> Result<(), ChannelError> {
        let work = async {
            let auth = self.api.call("auth.test", &json!({})).await?;
            let team = auth["team_id"].as_str().ok_or_else(|| {
                ChannelError::InvalidPayload("Slack auth.test omitted team_id".into())
            })?;
            let bot = auth["user_id"].as_str().ok_or_else(|| {
                ChannelError::InvalidPayload("Slack auth.test omitted user_id".into())
            })?;
            let has_owner = !self.owners.lock().await.is_empty();
            if let Err(error) = self.sync_commands(team, has_owner).await {
                tracing::warn!(%error, "Slack slash command sync failed; continuing Socket Mode with existing commands");
            }
            let (tx, mut rx) = mpsc::channel(128);
            let deliver = async {
                while let Some(event) = rx.recv().await {
                    let Some(event) = self.command_affixes.normalize(&event) else {
                        continue;
                    };
                    if self.handle_claim(&event, team, bot).await {
                        continue;
                    }
                    let owners = self.owners.lock().await;
                    let envelope = normalize_event(&event, team, bot, &owners);
                    drop(owners);
                    if let Some(envelope) = envelope {
                        self.messages
                            .inbound(&inbound, envelope)
                            .await
                            .map_err(|_| {
                                ChannelError::Transport("inbound receiver closed".into())
                            })?;
                    }
                }
                Ok(())
            };
            tokio::select! { result=socket::listen(&self.api,tx)=>result, result=deliver=>result }
        };
        tokio::select! { ()=shutdown.cancelled()=>Ok(()), result=work=>result }
    }
}
