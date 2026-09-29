# Project memory acceptance

The memory delivery is tested with real databases, files, Unix IPC, CLI processes
and host entrypoints. Model and embedding network responses use deterministic
mocks. No developer session, live API key or paid provider is needed.

## Reproduce

Use the repository's pinned Rust toolchain, Node 24+, Python 3.11+, and the normal
plugin dependencies. From an isolated checkout:

```sh
npm ci --ignore-scripts --prefix plugins/taskix-manager
cargo test -p agentix-memory -p agentix-task -p taskix --all-features
cargo test -p agentix-core task_board --all-features
cargo clippy -p agentix-memory -p agentix-task -p taskix -p agentix-core --all-features --all-targets -- -D warnings
node --test plugins/taskix-manager/tests/*.test.mjs plugins/agentix-bridge/tests/*.test.mjs
make test-backup
cargo fmt --all --check
cargo test -p agentix-memory --test performance -- --ignored --nocapture
```

The scale test is explicitly ignored in the ordinary suite to keep feedback
short. The CLI end-to-end test launches a real daemon, a local mock HTTP model,
and the Node prompt-hook fixture; plugin dependencies must be installed before
that Rust test. Memory service tests are Unix-only. Windows task-board support
is unaffected, but memory IPC is not implemented there.

## Coverage matrix

| Contract | Verification |
| --- | --- |
| Discussion and Job capture, immutable revision snapshots, unknown ownership, directory hints, attachment, legacy backfill, v14 migration, individual acknowledgements and restore forks | `agentix-task/tests/memory_sources.rs`, `agentix-core/src/engine/task_board/tests.rs`, `taskix/tests/cli.rs` |
| Chinese and identifier recall, project isolation, evidence validation, revisions, human edits, supersession, expiry and forget suppression | `agentix-memory/tests/store.rs` |
| Independent vector recall, generation/revision fences, invalid dimensions/values, partial indexing and query degradation | `tests/store.rs`, `tests/embedding_index.rs`, `tests/providers.rs` in `agentix-memory` |
| Fair claims, parallel extraction, serial project consolidation, lease expiry/retry, stale-source cancellation, atomic result application and bounded candidate batches | `agentix-memory/tests/queue.rs` |
| Fresh contexts, bounded steps/tools/context/time, strict model protocol, read-only scoped tools, repository audit and database availability during blocked model calls | `agentix-memory/tests/agent_loop.rs`, `tests/tools.rs`, `tests/providers.rs`, `tests/worker.rs` |
| Repository reviews retire documented agent memories; fabricated citations fail and concurrent human edits win | `agentix-memory/tests/worker.rs` |
| Host session/revision deduplication, same-turn packet retry, stale/forgotten filtering, legacy marker compatibility, late-response cancellation and fail-soft deadlines | `agentix-memory/tests/context.rs`, `plugins/taskix-manager/tests/memory.test.mjs` |
| Slow embedding falls back within the host deadline; a timed-out real IPC request does not suppress the next turn | `agentix-memory/tests/ipc.rs` |
| Private IPC permissions, single-instance ownership, concurrent requests, size limits, offline reads, config reload, deep-query citations and diagnostics | `agentix-memory/tests/ipc.rs`, `tests/api.rs`, `taskix/tests/support/memory.rs` |
| Full visible-turn → outbox → mocked Responses tools → consolidation → FTS → real CLI → Node prompt hook, with fresh consolidation context | `memory_visible_turn_to_mock_model_to_real_host_hook_end_to_end` in `taskix/tests/support/memory.rs` |
| Read-only note repair, body/metadata edit rejection, missing files, filesystem failure, symlink rejection and crash between file publication and receipt acknowledgement | `agentix-memory/tests/projection.rs`, CLI projection test |
| Ordered SQLite WAL snapshots, source-content coverage, single-format compatibility, retained upload retries, archive validation, atomic no-overwrite restore and no secret-bearing upload logs | `scripts/tests/test_taskix_backup.py` |
| Actual daemon restart from older memory, replay of already acknowledged receipts, real-schema archive/restore and fork rejection | CLI `memory_restore_replays_acknowledged_sources_and_rejects_a_forked_task_history` |
| Failed acknowledged replay input remains recoverable after restart; legacy gaps, persisted-source checks and stale checkpoint guards | CLI `memory_replay_retries_a_failed_acknowledged_receipt_after_later_receipts_succeed`, `agentix-memory/tests/store.rs` |
| Read-only Project lookup during an active writer, actual vector query-plan index, realistic scoped search during intake | `agentix-task/tests/memory_sources.rs`, `agentix-memory/src/vectors.rs`, `tests/performance.rs` |

The Obsidian memory guard is covered by
`plugins/taskix-manager/tests/obsidian-memory.test.mjs`: saved edits roll back,
normal projections stay quiet, Recovery files are excluded, concurrent edits
are rechecked, and lookup failure/unload prevents unsafe writes. Fake-clock tests
verify typing debounce, per-file notice cooldown and pending timer cancellation. The CLI
projection test verifies canonical document reads with the daemon stopped and
rejects foreign Project paths. Desktop Obsidian itself is not exercised.

The real daemon/CLI/Node hook fixture also discards a prepared context packet,
then verifies the next host turn still receives it and subsequent host turns
deduplicate it. Packet preparation itself never establishes host delivery.

## Measured scale

Local run on macOS arm64, Rust 1.95.0 **debug/test build**, September 29, 2026:

- 10,000 memories and vectors: 2,000 in the queried Project, 8,000 in another.
- 384-dimensional deterministic vectors, 256-row vector pages, top 8 results.
- 30 FTS and 30 hybrid searches while 100 source/queue writes run concurrently.
- FTS p50 **5.46 ms**, p95 **6.69 ms**.
- Hybrid p50 **29.63 ms**, p95 **38.63 ms**.

These are warm in-process storage measurements, including database contention
but excluding CLI startup, tokenizer cold start, IPC and remote query embedding.
They are not a production SLA or an ANN-scale claim. Exact cosine scanning grows
with the selected Project's vector count and dimensions. The test asserts scope,
bounded result pages and a generous two-second query deadline; measurements are
printed rather than enforcing a machine-specific millisecond threshold.

## Interpretation and operational limits

Mock success proves API/tool contracts, state transitions and recovery. It does
not measure whether a chosen live model consistently selects the right durable
decisions, refuses subtle prompt injection, or recognizes when a rationale is
fully documented. Repository search is bounded; incomplete coverage is exposed
to the model. Review requires a real literal file citation and revision guards,
but the equivalence judgment remains model-dependent.

The filesystem tests verify note bytes and SQLite reconciliation. They do not
prove desktop Obsidian rendering or file-watcher behavior on every platform.
Backup tests use a mock rclone executable; they do not establish live remote
credentials, scheduler installation or all filesystems' atomic-rename support.
Service/projection startup is opt-in and no user configuration is changed by tests.
Source snapshots, memory versions, old embedding generations and note Recovery
copies have no automatic retention policy. Budget storage and use paired backups;
only derived indexes are disposable. Restoring an old memory snapshot loses
human edits/forget operations made after that snapshot, even when newer task
conversation receipts can be replayed.

## Jev screening coverage

Worker tests verify explicit skip makes no model request, while extract and gate
failure call the model and preserve audit records. The real daemon/CLI mock-HTTP
test exercises enabled/disabled environment settings, an independent memory
confidence threshold even when the routing threshold is invalid, confidence fallback,
uncertainty, service errors and the resulting `routing metrics report` category.
Adapter tests cover configuration validation, input/output byte ceilings, timeout
and fail-soft metrics storage. Metrics tests cover v1 migration, rollback on failed
append, foreign database rejection and compact/JSON skip/extract/fallback reports.
These are protocol and state tests, not a measurement of live Jev recall.

Restored negative Jev gating covers low-confidence `extract`, low-confidence
`skip`, valid `uncertain`, malformed answers and service failures. All continue
to the model; only confident `skip` suppresses extraction. The calibration report
records the negative-gate backtest and why the positive experiment was rolled back.
