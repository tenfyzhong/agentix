# Agentix Product Design

## 1. Product definition

Agentix lets configured trusted owners inspect and control coding-agent sessions from IM clients while the agents continue running on the owner's machine. It is a control surface, not a hosted agent runtime: credentials, repositories, agent processes, and session logs stay local.

The first release supports:

| Area | Included | Deferred |
| --- | --- | --- |
| Agents | Codex, Pi, Oh My Pi | Claude Code |
| IM channels | Telegram, Feishu | Slack, Discord, others |
| Session operations | discover, attach, current, detach, history, create, fork, compact | archive/delete |
| Turn operations | start, persistent Codex follow-up queue, steer, stream, stop, model and reasoning settings | sandbox configuration |
| Human input | approvals, confirmation, free text, structured multi-question input with an `Other…` path | richer native form layouts |

## 2. Core user model

An Agentix instance selects one agent backend and may enable one or both IM channels. The same backend can have many concurrent sessions.

The core invariant is deliberately strict:

- One IM conversation has at most one current session.
- One session is current in at most one IM conversation.
- Switching while the old turn is active moves that old session into a draining state.
- A draining session receives only critical completion or interaction events; noisy stream deltas are suppressed.

This prevents two chats from steering the same session and prevents an old stream from overwriting the newly selected session's card.

## 3. Multi-session identity and display

Arrival order is never used for routing. Every normalized upstream event must carry:

```text
agent connection generation
session ID
turn ID
item ID (when applicable)
```

Agentix looks up the IM destination only by the exact session ID. Render state is keyed by `(session ID, turn ID)`, and platform message references are stored under the same key.

The visible hierarchy is:

```text
Codex · Parser cleanup · 7e52c1a9
Turn 019d… · Working 12s     short turn label, state, and elapsed time

👤 You                       user heading
> update the parser          quoted user context

🤖 Codex                     agent heading
> Working response…          quoted Markdown agent output
```

Each turn owns a separate IM message/card that is updated in place. Live turns, attach hydration, and `/history` share the same one-message-per-turn layout, with the user input and Markdown agent response shown in separate quoted sections beneath their respective headings. Only a short turn ID and a human-readable status appear in the header. Tool execution events and tool summaries are omitted from both live cards and history views so the conversation remains concise. Approval requests remain visible because they require an owner decision. A second session therefore updates a different message, even if events arrive simultaneously. Background completion and approval cards explicitly identify their background state.

Running turn status includes a working duration that advances once per second even when no agent delta arrives. The refresh edits the existing message/card and reuses its Stop action rather than issuing a new token. Terminal states stop refreshing and retain the final elapsed duration. Restored running turns measure elapsed time from the point Agentix observes them again.

## 4. Main flows

### Discover and attach

1. The owner sends `/sessions`.
2. Agentix lists each running session in a numbered quote block with a title, visual status, and monospace workspace path. Paths inside the current user's Home directory use `~`; internal session IDs are hidden from the picker.
3. The owner taps a button labeled with the session title or sends `/attach <id>`.
4. Agentix subscribes upstream, atomically persists the binding, and loads only the latest turn. If it is still running, it becomes the live turn card with a fresh Stop action. `/history` remains the explicit way to load older turns.
5. If that session was attached elsewhere, the old conversation receives a “session moved” notice.

### Attached session exits

When the underlying Codex process exits, Agentix marks any visible running turn as interrupted, removes its stale controls, sends a warning to the attached IM conversation, and temporarily detaches it. The desired binding remains durable. If `codex resume` starts the same session again, Agentix restores the subscription and IM attachment automatically and sends a success notification. Later duplicate exit or resume notifications do not produce duplicate IM messages.

### Background completion

When a turn finishes outside the currently attached session, Agentix identifies the completed session by title and short ID and offers a one-tap Attach action. A session that is draining after a switch updates its existing turn card; another unbound session creates one notification containing the completed turn's prompt and response for each authenticated conversation known to the running service. The standalone notices can be disabled with `notifications.background_turns = false`; existing cards still finish in place. Replayed terminal events are deduplicated. Completion in the current session remains part of its normal turn card and does not create a second notification.

### Prompt, queue, and steer

- If the selected session is idle, ordinary text starts a new turn.
- A Telegram or Feishu reply prefixes the new prompt with the earlier visible message content. Telegram prefers a manually selected quote over the full replied-to text or caption; Feishu resolves the parent message through OpenAPI. Only that earlier content is blockquoted, the new input stays ordinary prompt text, and slash commands do not include reply context.
- Unknown or malformed slash commands produce an IM warning containing the parsing error and the `/help` command list for the conversation's current attachment state. They are treated as handled input rather than retryable engine failures.
- If a Codex session has an active turn, ordinary text is persisted in its app-server FIFO queue as a separate follow-up turn. Agentix immediately returns the queue position, and `/queue` reads the current ordered queue.
- The Codex CLI Tab queue is independent from the app-server queue. Neither side synchronizes or deduplicates the other. When both contain pending input, they may both submit after the active turn ends, producing back-to-back turns with no shared ordering guarantee; users should not operate both queues concurrently for one session.
- Backends without persistent queue support continue to send ordinary text through their steering operation while a turn is active.
- Codex automatically starts the next queued message after a completed or failed turn. Interrupting a turn leaves its queue paused until Codex resumes it.
- The turn card is updated at most once per second during streaming, then flushed immediately at completion.

### Approval and input

- Approval buttons carry only opaque, random, single-use tokens.
- Tokens are bound to conversation, owner, and one in-memory action.
- After an approval decision succeeds, the original IM view removes every decision button and shows the selected option.
- Structured input renders one question at a time with buttons for declared options and an `Other…` path for free text. After the last answer, Agentix submits the complete question-ID map and replaces the controls with a summary.
- A request resolved by another app-server client removes its IM actions and pending free-text mode, then displays `Resolved outside Telegram`; the notification does not reveal the external decision or answers.
- Any backend disconnect invalidates old buttons and pending reply modes.
- Choosing `Other…` enters a reply mode for the current conversation; `/cancel` returns to that question's choices.
- Feishu callbacks are acknowledged by the SDK before the action is processed.

### Switch while running

The old session becomes draining and the new session becomes current. Old stream deltas are hidden. Its completion, failure, or urgent interaction is still delivered with a background-session label. A terminal update adds an Attach button before Agentix unsubscribes the old session.

## 5. Channel presentation

Telegram converts agent Markdown to MarkdownV2 in bounded UTF-8 text messages, registers a native command menu, and uses two-column inline keyboards. Feishu uses shared Card JSON 2.0 documents with a status-colored header, title/subtitle, Markdown body, and callback buttons. Because Feishu has no runtime API for per-conversation native bot menus, an interactive command card is sent after attachment and updated in place when attachment state changes. Both presentations are generated from the same channel-neutral `OutboundView` and `CommandMenu` models.

Status colors are semantic: blue for running, orange for waiting/warning, green for success, red for error, and grey for muted information, and purple for background turns. Feishu also gives background quoted content a tinted container; Telegram adds a ⚫ Background marker.

## 6. Security and privacy

- Default deny: only configured user IDs/open IDs are accepted.
- Groups additionally require a bot mention.
- Bot messages and unsupported message types are rejected.
- Channel credentials are stored directly in the local configuration file, which should be accessible only to its owner.
- Codex transport is Unix-domain-socket only in this release.
- IM event IDs are deduplicated durably.
- Action tokens contain no session ID, RPC ID, decision, or secret.
- Agent runtime approval and sandbox policies remain authoritative; Agentix does not weaken them.

## 7. Success criteria

- Two simultaneous sessions cannot cross-post deltas or overwrite each other's message.
- A switch during an active turn shows only critical old-session events.
- Graceful shutdown checkpoints durable bindings, removes IM controls, shows the detached menu, and announces that the service is offline without stopping the agent session.
- Restart announces that the service is online, restores still-running sessions and in-progress turn messages with fresh Stop actions, and discards saved bindings whose sessions are no longer attachable.
- Transport loss and a confirmed local Codex process exit do not remove a durable binding. Manual detach, replacement by another attachment, or a rejected startup reattachment does.
- A stale, repeated, cross-chat, or cross-owner button action is rejected.
- A transport reconnect reinitializes the protocol and restores subscriptions.
