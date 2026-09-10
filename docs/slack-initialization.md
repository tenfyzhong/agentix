# Slack Initialization and Integration Guide

Agentix uses Socket Mode to receive Slack messages, slash commands, and button callbacks without a public HTTP endpoint. During startup, the Slack adapter uses an authenticated Slack CLI to read the server manifest, merge the command menu, and update it when needed. The core engine receives unified commands such as `/sessions` and does not handle Slack management credentials or manifests.

## 1. Create and install the Slack app

1. Open [Slack app management](https://api.slack.com/apps), choose **Create New App → From scratch**, and select the target workspace.
2. Under **OAuth & Permissions**, add these Bot Token Scopes: `chat:write`, `commands`, `app_mentions:read`, `im:history`, `channels:history`, and `groups:history`.
3. Enable **Socket Mode**. Under **Basic Information**, record the App ID (`A…`) and create an App-Level Token (`xapp-…`) with `connections:write`.
4. Under **Event Subscriptions**, enable events and subscribe to `app_mention`, `message.im`, `message.channels`, and `message.groups`. Enable **Interactivity** as well. Socket Mode does not require public request URLs.
5. Under **App Home**, set the bot display name, enable the Messages tab, and allow users to send messages from that tab.
6. Install the app to the workspace and copy its Bot User OAuth Token (`xoxb-…`) from **OAuth & Permissions**. Reinstall after changing OAuth scopes.
7. Invite the bot to the channels where you want to use it. Each Agentix instance connects to one workspace.

You do not need to create, import, edit, or keep a manifest file, or register slash commands manually. Agentix generates the command configuration internally during startup and removes its temporary files after processing.

All three values must belong to the same app. The `xoxb` token sends messages and identifies the running bot; the `xapp` token establishes the Socket Mode connection; the App ID selects the app to synchronize. Do not put CLI management credentials in either token field.

### One command owner per workspace

Slack slash command names are workspace-wide, not scoped to an app or bot DM. If two installed apps register `/sessions` (or `/claim` or another Agentix command), Slack invokes the most recently installed command. Opening the intended bot's DM or choosing a matching autocomplete entry does not provide isolation between apps. An inactive app that owns the name can cause `/sessions failed because the app did not respond` while the running Agentix service receives no command. See Slack's [command naming rules](https://docs.slack.dev/interactivity/implementing-slash-commands/#naming-your-slash-command).

Before replacing a bot, remove the old app from the workspace or remove its conflicting slash commands. Keep only one installed app registering each Agentix command name. For parallel development or live integration tests, use a separate workspace; a different app name in the same workspace is not sufficient.

Agentix synchronizes only the configured app's manifest. It does not detect or remove commands owned by other apps. Clearing local bindings, restarting Agentix, or rotating tokens cannot resolve duplicate command registrations. Reinstalling the intended app may change precedence, but the conflict can return when the other app is installed again.

## 2. Log in to Slack CLI

Install the official [Slack CLI](https://docs.slack.dev/tools/slack-cli/), then run these commands as **the same operating system user that runs Agentix**:

```sh
slack version
slack login
slack auth list
```

Log in to the target workspace. The Slack user must own the app or be a collaborator with permission to modify and install it. This integration uses the command interface in Slack CLI **4.7.0**. A background service must have access to that user's CLI login state; logging in from another user's terminal does not authorize the service.

The CLI stores, uses, and refreshes its own management credentials. Agentix does not read the CLI credentials file, store an App Configuration Token, or pass management credentials through `--token`. Run `slack login` manually for initial authorization or after authorization is revoked. Agentix does not open an interactive login prompt during startup.

## 3. Configure Agentix

Keep your existing agent, storage, and other settings, and add or update:

```toml
# Optional global setting: place before the first [table].
# By default, Agentix looks for slack on the service process's PATH.
slack_cli_path = "/opt/homebrew/bin/slack"

[channel]
kind = "slack"

[channel.slack]
app_id = "A0123456789"
bot_token = "xoxb-your-bot-token"
app_token = "xapp-your-app-token"
owner_user_ids = ["U0123456789"]
```

`slack_cli_path` must be an absolute path to an executable, not a directory, shell alias, command with arguments, or `~/…` path. On Windows, use a TOML literal string such as `slack_cli_path = 'C:\Tools\slack.exe'`. Omit this option when the CLI is already on PATH. An explicitly configured path takes precedence.

With Slack selected and `app_id` configured, each Slack adapter startup attempts synchronization. For compatibility with older configurations, omitting `app_id` logs a warning and skips synchronization while keeping Socket Mode available. The workspace ID comes from the bot token's `auth.test` response and needs no separate setting. Selecting Telegram or Feishu does not start the Slack adapter or update its manifest, even if Slack settings remain in the file.

To initialize the owner list later, set `owner_user_ids = []`, start the service, and run `agentix client claim` locally. At startup, an empty owner list registers only `/claim` from Agentix and removes its normal command menu. Send `/claim <code>` in the bot's private conversation, without a leading space. After saving the Slack member ID, the adapter removes `/claim` and registers the normal commands, including `/sessions`, without a service restart. Claims sent in channels are ignored. Use a dedicated app: the unclaimed phase replaces the entire app slash-command list with `/claim`, while preserving other manifest settings.

Invalid or expired codes and owner-save failures keep the claim menu. If the owner is saved but the menu update fails, the owner remains linked; the bot reports the failure and the next service startup retries the normal menu. A configured owner list always selects the normal menu at startup and removes any stale `/claim` entry. Clearing the owner list and restarting returns the app to the claim menu.

## 4. What happens during startup

After you run `agentix serve`, the Slack adapter:

1. Authenticates with the bot token and obtains the workspace ID.
2. Creates a temporary Slack CLI project for this synchronization and links the configured App ID to the current workspace. Existing user projects are not modified.
3. Calls `slack manifest info --source remote` to retrieve the server manifest.
4. Selects `/claim` alone when the owner list is empty. Otherwise, generates commands such as `/sessions` and `/help` from the unified command catalog, while retaining `/agentix <command or prompt>`. When the task board is enabled, it also adds dashboard, board, jobs, inboxes, and inbox commands.
5. Replaces commands whose names occur in the unified catalog, removes legacy `/agentix-*` entries (regenerating aliases for the two reserved names), and retains the `/agentix` gateway. With an owner configured, other commands and settings are preserved; the unclaimed phase replaces the slash-command list with `/claim`. The required `commands` bot scope is added without removing existing scopes. Use a dedicated Slack app for Agentix; matching command names in that app are managed by Agentix.
6. Skips the update if command contents already match. Command order and omitted default fields do not trigger repeated updates. A missing `commands` scope still triggers an update. Before an update, it reads the server configuration again and aborts synchronization if it detects a concurrent change.
7. Writes the merged temporary manifest, calls `slack app install --force`, then retrieves the server manifest again to verify the commands.
8. Removes the temporary project, including the generated manifest, and starts receiving messages through Socket Mode. Cleanup also runs on errors, timeouts, and graceful shutdown.

The CLI calls explicitly pass `--app`, `--team`, `--no-color`, and `--skip-update`; linking also uses `--environment deployed`. CLI 4.7.0 has no standalone `manifest update` subcommand. The `app install` command updates the manifest and performs installation checks, which may be subject to workspace app approval rules. It does not deploy Agentix or make the CLI take over the Socket Mode connection.

The complete synchronization has a 60-second timeout. If the CLI is missing, authorization has expired, permissions are insufficient, the network fails, the manifest is invalid, installation fails, or verification fails, the adapter logs a warning and continues starting Socket Mode. A failed synchronization does not register new commands. Existing menus remain usable. For a temporary fallback, prefix `/sessions` with a space in the bot DM or use `@Agentix /sessions` in a channel. The next service startup attempts synchronization again. Successful owner enrollment also triggers synchronization; Socket reconnects do not repeatedly update the menu.

Synchronization can fail after the server has accepted the manifest, so failure does not guarantee that the server configuration is unchanged. The next startup uses the server's actual configuration. Slack manifest updates have no atomic version comparison, so avoid having multiple Agentix instances or administrators manage the same app concurrently.

Preservation is limited to manifest fields that the CLI can export and import. The CLI uses its own manifest schema; fields introduced by Slack but not recognized by an older CLI may not survive a round trip. Keep the CLI updated when using new app features.

## 5. Use and verify the integration

Type `/sessions` in Slack and select the Agentix command. No leading space or `agentix-` prefix is needed after successful synchronization. For example:

| Slack input | Input received by the core engine |
| --- | --- |
| `/sessions` | `/sessions` |
| `/agentix-rename My session` | `/rename My session` |
| `/agentix-status` | `/status` |
| `/agentix /help` | `/help` |
| `/agentix explain this error` | `explain this error` |

Slack rejects `/rename` and `/status` as app command names, so only these commands retain the `agentix-` prefix. `/agentix /rename ...` and `/agentix /status` also work.

Slack's native command list belongs to the entire app and does not change with a particular conversation's session attachment. The core engine checks the context for commands that require an attached session; the command reference inside the conversation still reflects its current state. Use bot mentions in Slack threads, where slash commands are not supported.

After initial setup, look for `Slack slash commands synchronized` in the logs. Run `/sessions`, select Attach, and send a prompt. After a restart with no menu changes, the log's `changed` field should be `false`.

## 6. Troubleshooting

| Message or symptom | Action |
| --- | --- |
| `cannot execute Slack CLI` | Check the service's PATH or configure an absolute `slack_cli_path`. Confirm that the file exists and is executable. |
| `configure channel.slack.app_id` | Set the target app's App ID in the Slack configuration. |
| `app_not_found` | Verify that `channel.slack.app_id`, `bot_token`, and `app_token` belong to the same app. When replacing a bot, update all three values, then restart Agentix. Also verify that the Slack CLI user can manage that app. |
| CLI exits with an error | Run `slack auth list` as the service user and `slack login` if needed. Check app collaborator permissions and workspace installation approvals. Agentix does not forward CLI output that might contain credentials; inspect the CLI's own logs for details. |
| `desc_too_long` | Shorten the app description in Slack Basic Information, then restart. A description accepted by the settings UI may still fail manifest validation. Agentix preserves your display text. |
| `/sessions is not a valid command` | Check the startup synchronization log. Ensure `app_id` is configured and fix any reported CLI or manifest validation error, then restart. An empty owner list intentionally exposes only `/claim`; complete enrollment first. Successful enrollment triggers the normal menu update. |
| `invalid manifest` / `Socket Mode must be enabled` | Check the server configuration, Socket Mode, and CLI version. |
| `manifest changed during synchronization` | Avoid concurrent configuration changes, then restart Agentix. |
| `timed out` | Check the CLI's network access, proxy, and login state, then restart. |
| Menu commands appear but do not respond | Check bot/app tokens, owner IDs, Socket Mode, event subscriptions, and channel invitations. |
| `/sessions failed because the app did not respond` | Check which app owns the command in Slack's command picker. A newer test app can shadow the running app's commands. Also check that the selected app has an active Socket Mode connection and that the logs contain no connection or ACK failures. |
| `chat.postMessage: channel_not_found` | The configured bot cannot access the destination conversation. Open the Agentix app's own DM, run `/sessions`, and attach the session there again. For a channel, invite the bot before attaching. Re-registering slash commands does not repair a conversation binding. |

Agentix checks the workspace and bot user identity before restoring bindings. Changing bots automatically clears old Slack routes and queued notifications; run `/sessions` in the new bot's conversation and attach again. Old routing data without a stored identity is deleted rather than migrated, including in released versions; reattach after upgrading. See [changing the configured bot](usage.md#changing-the-configured-bot).

With the same bot, an attached session keeps its destination conversation until it is detached or attached elsewhere. If that destination becomes inaccessible, working-state refreshes can repeatedly report the same send failure. A new attachment moves subsequent output to the new conversation; one final warning about notifying the displaced conversation can still occur. Check the timestamps to distinguish that warning from ongoing failures.

For diagnosis, compare the binding's `team:channel[:thread_ts]` with the workspace returned by `auth.test` and check the channel with `conversations.info` using the configured bot token and the required read scope. A `channel_not_found` response establishes that the bot cannot access that destination; it does not identify which conversation the user intended. Keep tokens out of logs. See Slack's [message destination rules](https://docs.slack.dev/reference/methods/chat.postMessage/).

Agentix's `[network].proxy` applies to its own HTTP/WebSocket requests. Slack CLI runs as a separate process and uses the environment proxy settings it supports. Agentix does not translate its TOML proxy configuration into CLI arguments.

If an old or test app also registers `/sessions`, follow [One command owner per workspace](#one-command-owner-per-workspace) to remove the collision. Selecting a bot DM does not select its slash command registration.

Development tests use reusable CLI, HTTP, and WebSocket fixtures without real workspace credentials. With the CLI installed locally, you can also run the local hook compatibility test, which does not update the remote app:

```sh
cargo +1.95.0 test -p agentix-slack --lib installed_cli_reads_generated_manifest_hook -- --ignored
```

Workspace permissions, installation approvals, and the actual Slack menu display still require verification during deployment. See also [Slack usage](slack.md), [CLI authorization](https://docs.slack.dev/tools/slack-cli/guides/authorizing-the-slack-cli/), [manifest commands](https://docs.slack.dev/tools/slack-cli/reference/commands/slack_manifest/), and [the install command](https://docs.slack.dev/tools/slack-cli/reference/commands/slack_app_install/).

## 7. Optional live synchronization test

The normal test suite uses a local CLI fixture. To verify the actual installed CLI
against a dedicated app in a separate workspace, explicitly opt in to the live test below. It updates the
selected app with the full command catalog (including task-board commands), verifies
the server manifest, and checks that the next synchronization makes no changes.
It does not send chat messages or restart the running service.

```sh
SLACK_LIVE_APP_ID=A0123456789 SLACK_LIVE_TEAM_ID=T0123456789 \
  cargo test -p agentix --test slack_live -- --ignored
```

To diagnose a command timeout, check `Slack Socket Mode connected` for the app ID and active connection count, then look for `Slack slash command received` and `Slack slash command acknowledged` with its elapsed milliseconds. These logs omit command arguments, message contents, and credentials. If the connected service never receives the command, check for other apps registering the same command and remove the collision. An ACK log records a completed local socket write; it does not prove that Slack received it.
