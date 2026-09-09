# Slack architecture and performance review

## Architecture review

Reviewed after implementing Slack, adding the transport-to-Codex integration test, and updating the setup and usage documentation.

| Area | Finding and disposition |
| --- | --- |
| Platform identity | Configuration duplicated the domain channel enum. Replaced it with a public alias and centralized display names in `ChannelKind`. Existing TOML remains compatible. |
| Owner bootstrap | Slack would duplicate Feishu's string-ID persistence boundary. Both now use the domain `OwnerClaimer` port; platform-specific credentials and IDs remain in their adapters. Telegram's numeric contract remains compatible. |
| Protocol isolation | Web API, Socket Mode, normalization, rendering, inbound bootstrap, and command-menu state have separate modules. Engine and SQLite do not parse Slack JSON or thread IDs. |
| Inbound responsiveness | Socket ACKs are independent of engine/outbound work. A bounded 128-event queue retains accepted envelopes. A full queue disconnects without acknowledging the unretained event, allowing Slack to retry. Backpressure and cancellation have transport tests. |
| Menu lifecycle | Initial implementation posted a new menu on every update. Regression test failed first; menus now update the existing message and skip identical updates. Missing messages are recreated. |
| Mention handling | Initial normalization removed every bot mention, including quoted requirement content. Regression test failed first; only a leading routing mention is removed. An edit can remove its routing mention if the previous message mentioned the bot. |
| Notification fallback | Block text was escaped but the fallback could retain Slack mention syntax. Regression test failed first; fallback is escaped too. |
| Persistence | Slack uses `team:channel[:thread_ts]` and string timestamps. SQLite reopen tests cover thread bindings and channel-scoped deduplication. An additive `channel_identities` migration protects routes from bot replacement; legacy bindings without a verified identity are cleared once at startup. |
| Integration | Local Socket Mode → Engine → Codex RPC → Web API tests cover attachment, prompting, command approval buttons, and final in-place updates. Critical core Stop and externally resolved interaction tests include Slack. |

The shared `ChannelAdapter` remains the appropriate boundary. A third-party framework or a generic protocol hierarchy would not remove Slack-specific authentication, block rendering, and event semantics; those stay within the Slack crate.

## Performance review

The review checks rendering cost, retained message state, API pacing, cancellation, and cross-conversation contention. Benchmark source: [render.rs](../crates/agentix-slack/benches/render.rs).

Run `cargo +1.95.0 bench -p agentix-slack --bench render`. Each input uses five warmups and 100 measured renders. Results are machine-local comparisons, not Slack latency measurements.

| Input | Before | After | Speedup |
| --- | --- | --- | --- |
| 4,088 bytes | 0.085 ms | 0.032 ms | 2.7× |
| 1,048,572 bytes | 6.976 ms | 0.414 ms | 16.8× |
| 8,388,604 bytes | 63.123 ms | 0.379 ms | 166.7× |

The bounded renderer stops reading once its block budget is filled, escapes entities without splitting them, and balances code fences at section boundaries. The fallback is built from an iterator with its own limit. It no longer copies or scans the full oversized body. Raw measurements are in [slack-render.json](benchmarks/slack-render.json).

Rendered action messages are retained instead of cloning the entire unbounded `OutboundView`. The cache retains at most 256 entries and 8 MiB of serialized payload content, with oldest-entry eviction; Rust container overhead is additional. Disabling controls reuses that payload rather than re-rendering it. Old evicted controls remain subject to core single-use token validation. Menu state is capped at 256 conversations.

New messages use a 1.1-second channel budget shared across threads; unrelated channels retain concurrency. Method-level `Retry-After` and channel budgets are checked together before each attempt, including after cancellation or a newly imposed cooldown. Tests cover pacing across threads, progress on another channel, cancelled retries, and a cooldown introduced during an existing channel wait. Stream updates retain their two-second minimum interval and API retries remain bounded.

## Limits

A successful Socket Mode ACK means an event is retained in memory, not durably written to SQLite. A process crash in the gap can lose it, as with the existing transport handoff model. This change does not introduce a durable broker. Normal core event claims still handle retries after delivery.

Tests use local services and do not verify real workspace installation, permissions, Slack availability, or a particular enterprise proxy. Follow [Slack setup](slack.md) for the real-workspace smoke test.
