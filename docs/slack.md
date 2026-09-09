# Slack

Agentix supports one Slack workspace per instance through Socket Mode. It receives private messages, channel mentions, edits, native slash commands such as `/sessions`, and Block Kit button actions without a public HTTP endpoint. The same session, approval, queue, and task-board flows used by other channels are available.

For the complete CLI login, initialization flow, and troubleshooting guide, see [Slack Initialization and Integration Guide](slack-initialization.md).

## Create and install the app

Create an app **From scratch** in [Slack app management](https://api.slack.com/apps), enable Socket Mode, configure its permissions and event subscriptions, and install it to your workspace. Follow the [initial setup steps](slack-initialization.md#1-create-and-install-the-slack-app) for the required console settings.

Record its App ID and Bot User OAuth Token (`xoxb-…`), then create an App-Level Token (`xapp-…`) with `connections:write`. Enable interactivity and writable app DMs, and invite the bot to each channel where you want to use it. Reinstall after changing scopes.

Agentix generates slash commands automatically at startup. You do not need to manage a manifest file: the adapter writes one only when an update is needed and removes the temporary project after processing, including failures and graceful cancellation.

The bot scopes are `chat:write`, `commands`, `app_mentions:read`, `im:history`, `channels:history`, and `groups:history`. Channel message subscriptions allow edited Inbox messages to be reconciled. Agentix filters ordinary channel text unless it mentions the bot. It does not fetch conversation history or download files.

The internal manifest handling and connection flow follow Slack's [manifest reference](https://docs.slack.dev/reference/app-manifest/) and [Socket Mode guide](https://docs.slack.dev/apis/events-api/using-socket-mode/).

## Command name conflicts

Use only one installed app per workspace for Agentix's command names. Slack routes duplicate slash commands to the most recently installed registration, regardless of which bot DM is open. When replacing a bot, remove the old app or its conflicting commands. Use separate workspaces for parallel test apps; renaming an app does not isolate `/sessions` or `/claim`. Agentix's manifest synchronization updates only the configured app and does not clean up other apps. See [One command owner per workspace](slack-initialization.md#one-command-owner-per-workspace).

## Configure Agentix

Keep your agent and storage settings and select Slack:

```toml
[channel]
kind = "slack"

[channel.slack]
app_id = "A0123456789"
bot_token = "xoxb-your-bot-token"
app_token = "xapp-your-app-token"
owner_user_ids = []
```

With `app_id` configured, startup uses the logged-in Slack CLI to fetch, merge, update, and verify the app's slash commands. Install Slack CLI 4.7.0 and run `slack login` as the service user. If it is absent from PATH, set the global absolute `slack_cli_path` before all TOML tables. Omitting `app_id` preserves legacy operation and logs a skipped-sync warning.

Both tokens are required. You can put Slack member IDs in `owner_user_ids`, for example `["U0123456789"]`, or bootstrap the first owner:

1. Start `agentix serve` using this configuration.
2. Run `agentix client claim` locally.
3. Send `/claim <code>` in the bot's private conversation.

A successful claim saves the sender's member ID to the configuration, consumes the code, and enables access immediately. Claims from channels, other workspaces, bot events, or an already initialized owner list are ignored. Claim codes never reach the coding agent.

When `owner_user_ids` is empty, startup registers only the `/claim` command for
Agentix. Successful private enrollment saves the owner and replaces `/claim` with
the normal menu automatically. If that update fails, the owner stays linked and
restarting the service retries the menu synchronization.

The global `network.proxy` applies to Slack Web API requests and WebSocket upgrades, including HTTP(S) and SOCKS proxies. No signing secret or incoming port is needed for this Socket Mode implementation.

## Use the bot

- Send `/sessions` (or `/agentix /sessions`), choose a session, and type a normal prompt in the bot DM.
- In a channel, mention the bot: `@Agentix /sessions` or `@Agentix explain this failure`. Only configured owners can operate it.
- Use `/agentix /help` to see commands. The text after `/agentix` is passed unchanged as an Agentix command or prompt.
- In a thread, mention the bot on every message. Each thread has its own attachment, separate from the parent channel and sibling threads. Replies remain in that thread. Slack does not support custom slash commands inside threads; use mentions there.
- Approval and plan-input buttons use the same single-use action tokens as other channels. Consumed controls are removed and the result is shown in place.
- Submit editable Inbox items as messages, such as `@Agentix /inbox investigate the failure`. Slack slash-command invocations are not editable message sources.

Thread replies route to their thread's session; they do not fetch or automatically quote the parent message. File-only messages are ignored. Long output is bounded to Slack's block limits with a visible truncation marker. Nonterminal stream and working-duration updates have a two-second minimum interval; final results bypass that interval.

See [Slack's slash-command documentation](https://docs.slack.dev/interactivity/implementing-slash-commands/) for its thread restriction.

## Troubleshooting

- `invalid_auth`, `not_authed`, or `token_revoked`: check the bot and app token fields; do not interchange them.
- `missing_scope`: update the app permissions, reinstall, and verify the app token has `connections:write`.
- `not_in_channel` or `channel_not_found`: invite the bot to that channel and use its DM conversation rather than a member ID as a destination.
- No inbound messages: check Socket Mode, event subscriptions, owner IDs, and the app's writable Messages tab. Channel messages must mention the bot.
- New messages are paced at 1.1 seconds per channel, shared across threads. A Slack rate-limit response pauses the affected API method according to `Retry-After`, with a bounded retry count. Socket disconnects reconnect with bounded backoff. Shutdown cancels connection attempts and pending delivery.

Automated tests use local Slack HTTP/WebSocket and Codex services. Workspace installation, granted permissions, and Slack service behavior still require a real-workspace smoke test with your credentials.
