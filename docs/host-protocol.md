# Local control and native host protocol

Agentix exposes two protocols on the same listener. Local clients make one request per connection. Native host extensions register once and keep a bidirectional connection open. Both use UTF-8 JSON objects delimited by a newline; neither is JSON-RPC 2.0.

The default listener is `unix://~/.local/share/agentix/control.sock`. `[server].endpoint` changes it; Pi/OMP extensions and the Claude plugin use `AGENTIX_CONTROL_ENDPOINT` for the corresponding override. Agentix owns the listener. Pi/OMP extensions connect from the original interactive process; Claude connects through its plugin MCP child. Neither starts a replacement agent process.

## Access and framing

The Unix socket has mode `0600`. Access is granted by local filesystem permissions, with no separate bridge token or credential file. Native registration additionally requires a configured backend and an absolute session file path under its configured session root. This is a same-user integration boundary, not isolation from other programs running as that user.

Native bridging supports Unix endpoints on macOS/Linux and numeric loopback TCP endpoints on all platforms. Set `AGENTIX_CONTROL_ENDPOINT=tcp://127.0.0.1:32198` for each host when using the Windows service default. TCP shares the ordinary local control trust boundary and does not add a separate bridge token. Do not expose a control endpoint through an untrusted proxy.

Each frame ends with `\n`. Limits apply per frame, including when several frames arrive in one socket chunk. The Node decoder accumulates fragments and assembles each completed frame once. Native frames are limited to 8 MiB, including the newline. Ordinary control requests have a 16 MiB limit. The first native/control handshake on a bridge-enabled listener has a three-second deadline. Extensions also wait at most three seconds for registration acknowledgement. Agentix's native RPC deadline is ten seconds, including waiting for and writing to the stream. An oversized response with a request ID becomes a bounded `frame_too_large` error; oversized events or registration frames close the connection. Backpressure can also cause disconnects. A failed or cancelled partial write invalidates the stream, preventing later requests from appending to a truncated frame.

## Local client interface

Send one request and read one response. There is no request ID on this interface:

```json
{"method":"sessions","params":{"cursor":null,"limit":20}}
```

```json
{"method":"session","params":{"operation":"send","session":"pi:native-id","text":"Continue the task","expected_turn":null}}
```

```json
{"ok":true,"result":{"turn_id":"turn-id"}}
```

Failures use `{"ok":false,"error":"description"}`. These errors are descriptive strings; native bridge error codes described below are a separate contract.

| Method | Parameters | Result |
| --- | --- | --- |
| `sessions` | `cursor`, `limit` | Normalized `SessionPage` with `sessions`, `next_cursor` |
| `session` | A `SessionOperation` from the table below | Operation-specific normalized result |
| `call` | `method`, `params` | Raw response from the existing Codex app-server connection |
| `claim` | `ttlMinutes` | Temporary owner claim information; intended for a local terminal |

| Session operation | Fields besides `operation` | Result |
| --- | --- | --- |
| `send` | `session`, `text`, nullable `expected_turn` | `{ "turn_id": "…" }`; a supplied turn selects steering |
| `stop` | `session`, `turn` | `{}` |
| `history` | `session`, nullable `cursor`, `limit` | Normalized `HistoryPage` |
| `command` | `session`, `command` | Normalized `SessionCommandResult` |

For example, a command value is `{"command":"model","params":null}` to list models, or `{"command":"rename","params":"New title"}`. The complete domain types are in [session.rs](../crates/agentix-domain/src/session.rs) and [agent.rs](../crates/agentix-domain/src/agent.rs). `SessionOperations` applies the same access and capability checks for local clients and IM. Native stop is session-scoped: Pi/OMP abort the current turn rather than matching a supplied turn identifier inside the extension.

CLI equivalents:

```sh
agentix client sessions
agentix client send pi:native-id 'Continue the task'
agentix client stop omp:native-id turn-id
agentix client history pi:native-id --limit 20
agentix client command omp:native-id '{"command":"model","params":null}'
```

The registry uses `SessionRef { agent, native_id }`. Public service IDs encode it as `pi:<native-id>`, `omp:<native-id>`, `claude:<native-id>`, or `codex:<native-id>`. Native IDs remain opaque, including any embedded colons. Unqualified IDs are accepted only when ownership is unambiguous. Extension frames and taskix provenance always use the native ID.

## Native registration, version 2

The extension sends this first frame:

```json
{"id":"register","method":"register","params":{"version":2,"agent":"pi","instance":"instance-uuid","pid":1234,"session_id":"native-id","cwd":"/work/project","session_file":"/home/user/.pi/agent/sessions/project/session.jsonl","snapshot":{"instance":"instance-uuid","seq":0,"session":{"id":"native-id","name":null,"preview":null,"cwd":"/work/project","updatedAt":1788880000,"status":"idle"},"capabilities":["prompt","steer","history","stop","queue","queue_control","status"]}}}
```

`snapshot` is a `SessionInfo`: metadata and capabilities only. It does not require history or a queue payload. A full legacy snapshot's additional fields are tolerated. `updatedAt` is Unix time in seconds and represents activity, not the time of a listing query. Its type, required nullable fields, and permitted statuses are defined in the schema.

Agentix replies `{"id":"register","ok":true}` on success. Invalid version, root, identity, backend, or duplicate live ownership can close the connection without a success acknowledgement. Do not send requests before acknowledgement. The `instance` and `snapshot.instance` must match, as must `session_id` and `snapshot.session.id`. Subsequent metadata cannot change either identity.

One live connection owns each `(backend, native session ID)`. Pending registrations reserve that identity without holding the shared registry lock across socket writes; cancellation releases the reservation. A new native session or fork uses a fresh instance UUID. Reconnecting the same extension preserves its instance and event sequence. Hub registration emits the service's resume lifecycle event; extensions must not emit a second resume event. Disconnect removes that owner and emits offline once. Detaching an IM conversation leaves the native connection and terminal alive. Service shutdown closes connections; the default extension retries every second while active. No reconnect automatically resubmits a prompt.

For diagnostics, a short-lived connection can send:

```json
{"id":"inspect","method":"inspect","params":{"version":2,"agent":"pi","session_root":"/home/user/.pi/agent/sessions"}}
```

The response is `{"id":"inspect","ok":true,"result":{"count":1}}`. The root must match the configured root to produce a nonzero count. Diagnostics query the existing listener and never bind another socket.

## Requests and responses on a registered connection

Agentix sends `{"id":"unique-rpc-id","method":"info","params":{}}`. The extension answers `{"id":"unique-rpc-id","ok":true,"result":{…}}`, or `{"id":"unique-rpc-id","ok":false,"error":"description","code":"busy"}`. Correlate responses by ID; responses and events may interleave. An RPC ID identifies a transport exchange. A delivery `request_id` identifies a logical prompt across retries.

| Method | Parameters | Result and behavior |
| --- | --- | --- |
| `info` | `{}` | `SessionInfo`; listing uses this without scanning history or copying the queue |
| `snapshot` | `{}` | `Snapshot`: metadata, latest history page, queue view |
| `history` | Optional `cursor`, `limit` | `History`: `turns`, `older_cursor`, `newer_cursor` |
| `prompt` | `text`, `request_id` | `PromptResult`; starts a turn only when idle |
| `steer` | `text`, `request_id` | `PromptResult`; steers an existing active turn |
| `stop` | `{}` | `{}`; pauses the remote queue and aborts the host's current turn |
| `queue` | `text`, `request_id` | `QueueItem`; records FIFO work and schedules delivery when idle |
| `queue_state` | `{}` | `QueueState`: `items`, `paused`, `uncertain` |
| `queue_resume` | `{}` | `{ "message": "Queue resumed" }`; rejects uncertain delivery |
| `queue_clear` | `{}` | A message; removes pending/uncertain work, retaining an active delivery if busy |
| `command` | `name`, optional/nullable `value` | `CommandResult`: `body`, `choices` |

Pi/OMP commands include `status`, `model`, `reasoning`, `rename`, `compact`, `skills`, and `diff`, when their public APIs support them. Choices contain `label` and `value`; send the selected value through the same command. Advertise actual capabilities, using names from `SessionCapability`. Current remote queue capability names are `queue` and `queue_control`. Unknown names do not grant capabilities. Local third-party dialogs remain in the terminal; there is no bridge approval-response interface. IM exit/detach is a service operation, not a host process termination command.

Pi/OMP history pages contain at most 20 turns. Cursors are branch-relative offsets encoded as strings; they are not stable database snapshots across branch changes. Text longer than 100,000 characters is visibly shortened and item lists retain the latest 20 entries per turn. A stable native leaf ID permits reuse of the branch index; leaf changes, host activity, or turn association invalidate it. Hosts without a leaf API rebuild the index. Listing queries up to 16 connections concurrently and sorts summaries before applying its own pagination.

For Pi/OMP, `info`, `snapshot`, `history`, `queue_state`, and `stop` bypass unrelated pending extension commands. Other extension mutations are serialized. This is an extension scheduling guarantee, not a bound on IM, service-handler, provider, or network latency.

## Events and recovery

An event has no RPC ID:

```json
{"instance":"instance-uuid","seq":1,"event":{"TurnStarted":{"session_id":"native-id","turn_id":"turn-id"}}}
```

| Event | Payload fields |
| --- | --- |
| `TurnStarted` | `session_id`, `turn_id` |
| `AgentMessageDelta` | `session_id`, `turn_id`, `item_id`, `delta` |
| `ItemStarted` | `session_id`, `turn_id`, `item_id`, `kind`, `label` |
| `ItemCompleted` | `session_id`, `turn_id`, `item` |
| `TurnCompleted` | `session_id`, `turn_id`, `status`, nullable `error` |
| `QueueChanged` | `session_id` |
| `SessionExited` | `session_id` |

`seq` increases by one per emitted event, beginning after the registration snapshot's sequence. Duplicate/older sequences and foreign instances are ignored. A gap, malformed payload, or foreign session invalidates the connection. Reconnect establishes a fresh metadata baseline; history reconciles missed output. The service also reconciles active bindings if its internal event broadcast lags, invalidating stale interactive actions first.

Only signal completion after the host settles. Pi uses `agent_settled`; OMP intermediate `agent_end` events with `willContinue=true` or `isTerminal=false` do not finish a turn. Tool and text events may precede the response to a prompt request.

Delivery state belongs to the extension's original native session. The Pi/OMP implementation appends `agentix.bridge` custom entries containing incremental queue mutations, pause/in-flight state, receipts, and turn association. It can replay older full checkpoints followed by incremental records. Append must succeed before acknowledging a delivery or emitting its start. A failed append leaves queue ownership and receipts unchanged; native storage controls the physical durability guarantee.

A repeated delivery ID with the same operation and text returns its original result or failure without submitting again. Reusing it for different content fails. Receipts are retained for the session lifetime, including after clearing pending work. Reload with an in-flight delivery marks it uncertain and pauses the queue: inspect history, clear explicitly, and submit a new request if needed. This avoids automatic duplicate execution but is not an exactly-once provider guarantee. A fork does not replay another session's queue. Native terminal queues remain separate.

Stable failure codes are `invalid_request`, `unsupported_method`, `unsupported_command`, `busy`, `delivery_uncertain`, `host_error`, `frame_too_large`, and `session_changed`. Do not classify failures by their prose. A timeout or lost response does not prove that a prompt was not accepted. Agentix does not automatically retry it; a lower-level client retaining the delivery ID can query/retry according to the deduplication contract.

## Adding another host

Use the [schema](../plugins/agentix-bridge/protocol/schema.json) as the wire source of truth. [wire.ts](../plugins/agentix-bridge/protocol/wire.ts) and [wire.rs](../crates/agentix-bridge/src/wire.rs) are generated by `node plugins/agentix-bridge/protocol/generate.mjs`; `--check` verifies freshness. The checked-in snapshot fixture and Node/Rust contract tests validate serialization and explicit domain conversion.

A new host needs an adapter tied to the original session for native identity, activity, events, history, prompt delivery, and supported controls. Reuse the transport/session/queue modules when their native semantics fit. Register its backend identity and configuration in the service; the current accepted backend kinds are not dynamically extensible by merely sending an arbitrary name. Keep protocol DTOs at the adapter boundary and convert to domain types explicitly. Add contract, reconnect, isolation, uncertain-delivery, and original-process integration tests before enabling the host. See [architecture](architecture.md), [integration coverage](integration-coverage.md), and [benchmarks](performance.md).

## Claude Code

The `claude` backend uses the same version 2 transport through its MCP plugin, with rmux prompt delivery by default and an explicit Channel alternative. See [Claude Code](claude-code.md) for registration, hook identity, explicit delivery acknowledgement, history, and supported capabilities. MCP is used only between Claude and the plugin, not on the Agentix socket. Claude advertises `prompt`, `history`, `status`, and `queue_control`; other methods in the table are not implied capabilities. Its history pages also contain at most 20 turns. Hooks and reads remain responsive during the seven-second prompt acknowledgement wait. Claude has no remote FIFO: queue methods inspect or clear uncertain receipts, and another prompt while busy is rejected.

### Multiplexer configuration on registration

Successful registration returns `result.multiplexer.kind` (`rmux`, `tmux`, or `null`) from the startup detection result. `null` means terminal management is disabled. Clients refresh this value on every registration, including reconnects. Claude delivery mode `multiplexer` requires an enabled backend before accepting prompts; a `null` value clears terminal delivery without rejecting registration; `auto` verifies its inherited terminal context independently.
