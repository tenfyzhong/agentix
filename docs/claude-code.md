# Claude Code plugin bridge

The user guide has moved to the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki/Claude-Code). Start with the [Wiki home](https://github.com/tenfyzhong/agentix/wiki/Home) for installation, configuration, and everyday use.

The implementation and development notes below remain in the repository.

## Protocol and lifecycle

The registration acknowledgement includes `result.multiplexer.kind` from the running service startup detection (`null` when disabled). The plugin does not reread a separate TOML file. The plugin reuses bridge protocol version 2, newline-delimited JSON registration, requests, responses, snapshots, and events on the existing control socket. MCP JSON-RPC is confined to the Claude-to-plugin stdio connection. No additional socket listener is created.

Exec-form hooks and the MCP server run as children of the same Claude process. They exchange hook events through a private mailbox keyed by parent PID and process start time, so two terminals in the same working directory remain separate. Filesystem notifications wake the consumer; a one-second fallback scan covers unavailable or missed notifications. An event is removed only after handling succeeds, so a failed state write leaves it and subsequent events available for retry. SessionStart supplies the real session ID, transcript path, and working directory before or after MCP startup. A session switch replaces the bridge incarnation. Agentix reconnection preserves it. Reconnecting Agentix while the plugin remains running reconciles the turns observed by that plugin. The private store is not a continuous mirror of edits made to the transcript while the plugin and its hooks are absent; inspect native history locally if activity was not observed.

The default state directory is `~/.local/share/agentix/claude`; `AGENTIX_CLAUDE_DATA_DIR` overrides it. `AGENTIX_CONTROL_ENDPOINT` overrides the Agentix control endpoint, as for Pi/OMP: use `unix:///custom/control.sock` or `tcp://127.0.0.1:4150`, matching `[server].endpoint`. State files contain conversation text and delivery receipts and are created with owner-only permissions. Claude transcript files are read, never modified. On first registration the plugin imports up to 16 MiB of the transcript tail, excluding an incomplete final JSONL record. Subsequent observed turns and receipts are saved independently for recovery. Existing `sessions/<hash>.json` checkpoints remain readable; changes are appended to the adjacent `.jsonl` journal instead of rewriting all history and receipts on every hook. Startup replays the journal and truncates an incomplete final append; malformed complete records fail explicitly. These files retain conversation text and deduplication receipts for the session lifetime. Back up both files together. Local filesystem writes are synchronous but do not issue a per-record `fsync`; this is process-restart recovery, not a power-loss durability guarantee.

The supported capabilities are `prompt`, `history`, `status`, and `queue_control`. Stop, steering, remote model changes, approval relay, and queued prompt submission are not advertised. Queue controls only reconcile uncertain deliveries; they do not manage Claude’s native event queue. Replies are delivered through completion hooks (or the reply tool in Channel mode), rather than token-by-token streaming. The terminal remains the place to interrupt Claude or handle permissions.

The delivery interface is `send({ request_id, text, signal })`; `TerminalDelivery` is the default implementation and `ChannelDelivery` is an explicit alternative. Transport writes do not acknowledge delivery. The bridge waits up to seven seconds for the matching prompt hook (terminal delivery) or explicit tool acknowledgement (Channel) before returning `delivery_uncertain`. It retains the receipt and does not resend automatically, including after a restart. Late acknowledgements reconcile the original request. An unresolved delivery prevents another remote prompt; inspect the session and `/status`, then use `/queue clear` to explicitly abandon the uncertain receipt before submitting a new request. This does not cancel input already submitted to Claude. A reply tool call updates the answer but does not complete the turn, because Claude may continue using tools afterward.

## Development and verification

```sh
npm ci --ignore-scripts
node plugins/agentix-bridge/claude/build.mjs --check
node --test plugins/agentix-bridge/tests/claude*.test.mjs
AGENTIX_TEST_NATIVE_HOSTS=1 node --test plugins/agentix-bridge/tests/claude-native.test.mjs
cargo test -p agentix-bridge
```

The native smoke test installs the plugin into an isolated Claude configuration and verifies registration from the original host PID. It uses an unreachable local model endpoint and does not call an external model service. Protocol tests exercise acknowledgements, replies, completion, and the real Rust bridge adapter without model inference. To update the bundled MCP SDK after changing its locked development dependency, run `node plugins/agentix-bridge/claude/build.mjs` and commit the generated bundle.

References: [Channels](https://code.claude.com/docs/en/channels), [Channel protocol](https://code.claude.com/docs/en/channels-reference), and [hooks](https://code.claude.com/docs/en/hooks).
