# Project memory acceptance

The memory delivery is tested with real databases, files, local IPC, CLI processes
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
that Rust test. Shared memory and CLI tests run on Unix and Windows. Windows CI
also runs TCP exclusivity, concurrent-client, shutdown/rebind and unavailable
service tests. Platform-independent TCP tests also exercise endpoint discovery,
loopback enforcement, concurrent clients, stale endpoint recovery and refusal
to send business data to a different service instance on Unix. Unix permission checks remain Unix-specific. Cross-compilation
checks types and platform gates; it does not replace native Windows execution.

The projection file tests cover platform-specific directory sync and successful
initial publication. Recovery verification tests cover a full unordered page of
100 receipts, duplicate sequences and changed historical content. Native Windows
execution is still required to validate filesystem behavior beyond type checks.

## Coverage matrix

| Contract | Verification |
| --- | --- |
| Unix service login credentials, shell unset, startup noise, malformed/missing framing, failed lookup, three-second timeout, one lookup across reload and SIGTERM shutdown of the original PID | `taskix/tests/support/memory_login.rs`; parser byte-preservation tests in `taskix/src/memory/login_environment.rs`. CLI fixtures use isolated mock shells; Windows retains its launcher environment |
| Optional reasoning configuration, invalid values and Responses/Chat Completions field mapping | `agentix-memory/tests/providers.rs` |
| Windows local TCP exclusivity, concurrent clients, service absence and instance greeting | `agentix-memory/tests/windows_ipc.rs`, shared `ipc.rs` and CLI daemon tests; Windows CI |
| Discussion and Job capture, immutable revision snapshots, unknown ownership, directory hints, attachment, legacy backfill, v14 migration, individual acknowledgements and restore forks | `agentix-task/tests/memory_sources.rs`, `agentix-core/src/engine/task_board/tests.rs`, `taskix/tests/cli.rs` |
| Chinese and identifier recall, project isolation, evidence validation, revisions, human edits, supersession, expiry and forget suppression | `agentix-memory/tests/store.rs` |
| Upgrade either schema 15 layout to schema 16 while preserving existing Jobs, memory source identity and pending receipts, and backfilling event watermarks | `schema_fifteen_*` in `agentix-task/src/persist_tests.rs` |
| Independent vector recall, generation/revision fences, invalid dimensions/values, partial indexing and query degradation | `tests/store.rs`, `tests/embedding_index.rs`, `tests/providers.rs` in `agentix-memory` |
| Historical evidence suppression after merge and reopen; discovered-dimension mismatch backoff and continued progress through split batches | `agentix-memory/tests/lifecycle_regressions.rs` |
| Long host prompts, per-record embedding failure isolation, persisted retry limits, explicit retry and revision/generation reset | `agentix-memory/tests/retrieval_failures.rs` |
| Fair claims, parallel extraction, serial project consolidation, lease expiry/retry, stale-source cancellation, atomic result application and bounded candidate batches | `agentix-memory/tests/queue.rs` |
| Fresh contexts, bounded steps/tools/context/time, strict model protocol, read-only scoped tools, repository audit and database availability during blocked model calls | `agentix-memory/tests/agent_loop.rs`, `tests/tools.rs`, `tests/providers.rs`, `tests/worker.rs` |
| Repository reviews retire documented agent memories; fabricated citations fail and concurrent human edits win | `agentix-memory/tests/worker.rs` |
| Missing registered roots are unavailable, restored directories become usable again, invalid file roots still fail, and periodic reviews clear obsolete missing-root errors without scheduling work | `repository_roots_*` in `taskix/src/memory/daemon_tests.rs` |
| Successful worker progress clears its last background failure while idle probes preserve it | `worker_errors_clear_after_success_but_not_after_idle_probes` in `taskix/src/memory/daemon_tests.rs` |
| Host session/revision deduplication, same-turn packet retry, stale/forgotten filtering, legacy marker compatibility, late-response cancellation and fail-soft deadlines | `agentix-memory/tests/context.rs`, `plugins/taskix-manager/tests/memory.test.mjs` |
| Slow embedding falls back within the host deadline; a timed-out real IPC request does not suppress the next turn | `agentix-memory/tests/ipc.rs` |
| Private IPC permissions, single-instance ownership, concurrent requests, size limits, offline reads, config reload, deep-query citations and diagnostics | `agentix-memory/tests/ipc.rs`, `tests/api.rs`, `taskix/tests/support/memory.rs` |
| Reload preserves deep-query and provider admission across live requests; cancellation releases capacity and live limit reductions retire permits | `taskix/src/memory/daemon_tests.rs`, `agentix-memory/src/providers/http.rs` |
| Missing database parent directories remain untouched while doctor/status report uninitialized state | `memory_diagnostics_handle_a_missing_database_directory_without_creating_it` in `taskix/tests/support/memory.rs` |
| Paused receipt publication cannot both discard the current injection and suppress the next turn | `plugins/taskix-manager/tests/memory.test.mjs` with controlled filesystem completion and mocked deadline |
| Full visible-turn → outbox → mocked Responses tools → consolidation → FTS → real CLI → Node prompt hook, with fresh consolidation context | `memory_visible_turn_to_mock_model_to_real_host_hook_end_to_end` in `taskix/tests/support/memory.rs` |
| Read-only note repair, body/metadata edit rejection, missing files, filesystem failure, symlink rejection and crash between file publication and receipt acknowledgement | `agentix-memory/tests/projection.rs`, CLI projection test |
| Complete content and reason in the Markdown body, metadata-only frontmatter, and legacy note layout repair without database or revision changes | `projection_keeps_complete_content_and_reason_in_body_only`, `projection_rewrites_legacy_frontmatter_without_changing_database_revision` in `agentix-memory/tests/projection.rs` |
| Ordered SQLite WAL snapshots, source-content coverage, single-format compatibility, retained upload retries, archive validation, atomic no-overwrite restore and no secret-bearing upload logs | `scripts/tests/test_taskix_backup.py` |
| Actual daemon restart from older memory, replay of already acknowledged receipts and fork rejection on all platforms; real-schema archive/restore through the Unix backup script, and SQLite snapshots on Windows | CLI `memory_restore_replays_acknowledged_sources_and_rejects_a_forked_task_history` |
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

## Maintenance optimization coverage

- `agentix-memory/tests/worker.rs` verifies rapid source revisions make no model
  request during the settling window, and supersession cancels a blocked model
  future while preserving the replacement work and original source.
- `taskix/src/memory/daemon_tests.rs` holds one Project's HTTP request open while
  two other Projects make progress, checks concurrency limits of one and two,
  and verifies reload prevents the old batch from publishing vectors.
- `agentix-memory/tests/projection.rs` uses an update trigger to prove unchanged
  publication receipts receive no writes. Pending-only publication handles new
  revisions; full sync still repairs external edits, missing files and crashes.
- `agentix-memory/tests/source_index.rs` rebuilds the derived turn index from an
  older schema, preserves order across revisions, and checks indexed pagination
  without a temporary sort. Existing tool tests retain paging and scope checks.

- `agentix-memory/tests/api.rs` removes the FTS table after preparing a packet
  to prove same-turn retries bypass retrieval, including expired/empty packets.
- `agentix-memory/tests/context.rs` verifies unchanged receipts do not fire an
  update trigger, bounded background cleanup preserves recent receipts, and
  committed changes notify subscribers sharing a cloned store. The context module
  also checks that batch validation uses an ID lookup instead of a Project scan.
- `agentix-memory/tests/query_coalescing.rs` counts mock HTTP calls for overlapping
  queries, independent short/long deadlines, failures and Project/generation keys.
- `taskix/src/memory/daemon_tests.rs` also checks bounded idle backoff and an
  idle Project receiving a committed update while another Project remains slow.
- `agentix-memory/src/vectors.rs` compares heap results against full sorting
  across capacities, ties and arrival orders, checking the retained size bound.

These are behavioral and query-plan checks, not a new wall-clock benchmark or a
measured provider-cost reduction. The scale numbers below predate these changes.

## Memory regression coverage

Source-context regressions cover cross-turn source discovery and pagination,
session/Project isolation, stable order after a prior turn is revised, and a
worker resolving a user choice against a preceding legacy proposal. Store tests
accept user selection plus assistant supporting evidence while retaining the
assistant-only rejection. `retrieval_failures.rs` covers long prompts, failed
embedding batch isolation, durable backoff/retry limits, explicit reindex and
revision/generation changes. A real CLI/Node fixture verifies a long
identifier-rich prompt still receives memory. Offline status also reads older
v1 snapshots without the additive embedding retry table.

`agentix-memory/tests/lifecycle_regressions.rs` covers:

- Forgetting a merged memory suppresses both its original and current evidence;
  reopening the database preserves both suppressions.
- With dimensions omitted, a response inconsistent with the resolved profile
  reports a failure, records retry state, and lets a later valid memory proceed.
- After an input-rejected batch is split, one individual dimension mismatch
  enters backoff without preventing the remaining valid record from being indexed.
- Consolidation retains literal candidate quotations, including valid covering
  quotations from the same source, and rejects fabricated extensions.
- Direct consolidation archival is rejected without mutating the target; the
  dedicated repository-review tests retain valid and invalid citation coverage.

These tests exercise existing stored version history and deterministic HTTP
responses. They do not claim that upgrading retroactively repairs suppression
records for memories already forgotten by an older build. Review-only tests
archived outside the checkout are not part of the reproducible suite or coverage
matrix; the normal suite is defined by the repository test files.

## Consistency and compaction coverage

`tests/consolidation_consistency.rs` covers partial replacement while retaining
unrelated deployment facts and original evidence, atomic rollback on stale
related revisions, evidence-loss rejection, both sides of unresolved conflicts,
explicit forgetting with replay suppression, negated-request rejection, and
corrective feedback when a supplied related record is left unassessed, and
shared read-only assessments following a sibling candidate mutation.

`tests/compaction.rs` covers bounded Project-scoped manual pages, deduplication
after restart, write debounce, protected human/inactive records, persisted
historical progress, no duplicate seed or recursive scheduling, and stale seed
completion with zero model calls, terminal-failure suspension across work-retention cleanup, compact-specific
current-state reconciliation, and restoring seed evidence before validation.
`tests/atomic_facts.rs` covers structured identity, write-time replacement with
independent sibling facts unchanged, evidence merging, one-active uniqueness
under simultaneous writers, original-source ordering, immutable values,
qualifier normalization, stale revision rollback, human protection, active-only
retrieval and separate conflict diagnostics. Legacy split regressions verify
path/autostart/domain separation, complete evidence and version preservation,
visible `derived_from` lineage, direct proposals from preloaded snapshots without read tools, bounded validation
corrections with a lower user budget preserved, no recursive work, all-or-nothing rollback,
corrective feedback for altered literal quotes and current-conclusion migration scope for retained legacy records, snapshot-bound preloaded quote references with explicit valid-index feedback, expanded-submission byte limits, assistant-only decision attribution, missing-quote reference feedback, duplicate-create correction, snapshot-bound related assessments, and one-time schema 2-to-3
migration including explicit null facts. Backup tests preserve schema 3 and reject
unsupported schema 4; CLI restore and host extraction tests exercise the new schema. A worker regression rejects unstructured
extraction and then writes all three facts as settled. Existing low-level legacy
merge regressions retain compatibility for already queued work. API, provider and
Agent-loop tests cover safe error classification, bounded tools, permanent HTTP
rejections and final-step submission. Tool regressions distinguish a model-guessed
missing repository path (correctable input) from operational I/O failures. Taskix CLI tests cover command discovery.

The write-driven scheduler regressions also verify that unchanged compacted
revisions stay settled across days and restart, historical dirty work drains in
batches of ten, inflight old seeds wake newer revisions on completion/failure/lease
expiry, and the repository-review loop does not scan semantic compaction.
`taskix/src/memory/daemon_tests.rs` verifies write notification and debounce
scheduling, disabled/re-enabled recovery, startup history, reload notification,
and advances the clock by two idle days after removing the compact
table to prove that no idle compact SQL is executed. This isolates the compact
loop; source intake, embedding, projection and repository review have independent
schedules.

`examples/compaction_acceptance.rs` is an opt-in live acceptance harness. It opens
the original database read-only, copies one bounded Project page and its source
evidence into a temporary database, schedules only still-mixed records for real
configured-model splitting (existing atomic facts remain available), then
asks for the current REALITY domain. Acceptance requires every effective record
to contain one structured fact, at most one active version per identity, exactly
one effective domain fact, old mixed originals superseded, complete split lineage,
all original literal evidence and versions preserved, and a sourced current-value
answer. Other attributes such as SNI may legitimately contain the same hostname.
It never modifies the original Project or installs/restarts the service.
Deterministic tests establish transaction and queue contracts; this harness checks
model judgment on a concrete case, not general accuracy or throughput.

The earlier aggregate-record acceptance is superseded by this atomic-fact
contract. Its four-to-two result is not evidence for atomic separation. The
reported HTTP 400 was an upstream unsupported-egress-location rejection; requests
recovered after the user repaired the proxy route. Application changes improve
bounded diagnostics and validation, not provider geographic eligibility.

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


## Query admission, idle workers and retention

- `tests/context.rs` holds an unrelated SQLite writer while a cache miss returns
  through the read-only path; invalid identities and budgets remain rejected.
- `providers/http.rs` verifies reserved query admission, the shared total cap,
  and a waiting query preceding the next batch when concurrency is one.
- `tests/queue.rs` verifies post-commit queue notifications and duplicate-intake
  silence; daemon tests verify idle deadlines and notification reset.
- `tests/maintenance.rs` verifies obsolete-data cleanup, the first-observed
  retention clock, source/version/audit preservation, indexed review references,
  per-pass bounds and continued cleanup after reopening.

[Current performance measurements](taskix-memory-performance.md) include debug
and release results, provider request counts and scheduler CPU scope. These
manual benchmarks supplement deterministic tests; they are not production SLOs.

### Optional subscription-backed copy acceptance

`compaction_acceptance` copies a complete small Project into a temporary database,
checks atomic facts, original evidence, superseded legacy records and version history,
and asks for the replacement domain through the same memory query loop. The original
database and installed memory daemon are unchanged. Its default model uses the configured
HTTP provider. To use an existing ChatGPT Codex login for acceptance, select Luna explicitly:

```sh
cargo run -p agentix-memory --example compaction_acceptance -- \
  CONFIG PROJECT_ID RETIRED_VALUE CURRENT_VALUE --codex-luna
```

This opt-in example uses `gpt-6-luna` through standalone `codex exec`, without the
shared daemon or inherited parent session identifiers, with personal configuration,
plugins and native shell, web and multi-agent tools disabled. It returns a nonempty array of external call objects for the outer memory loop
to execute; these labels do not need to exist as native Codex tools. Each response
is constrained by the actual tool schemas, with typed arguments rather than
JSON encoded inside strings. Older replay replies remain decodable. The Luna option
loads the selected model metadata from the existing Codex model cache into its
temporary directory; missing selected metadata is an explicit setup error. Request and task deadlines remain those of the
supplied memory configuration; this flag does not add a production subscription provider
or change the daemon's provider. The extraction benchmark reuses the bridge with its
existing pinned model, replay checkpoint and request budget.

Before a legacy proposal reaches the fenced write, the worker checks each proposed
new identity against the active-fact index (at most sixteen metadata lookups).
Correction feedback contains bounded IDs, revisions, actor and same-value flags,
including matches outside the initial lexical snapshots. It does not reload full
quotation bodies. Database failures stop the attempt instead of asking the model
to correct infrastructure errors. The transaction repeats its uniqueness and
revision checks to protect against intervening writes.
Untouched atomic and human snapshots do not require redundant `keep` assessments;
their records remain byte-for-byte unchanged. Supplied assessments are still
validated, atomic mutations need guarded part actions, and every related legacy
agent record still requires an explicit assessment or targeted reconciliation.

For an explicitly authorized one-shot migration, `compact_codex` executes selected
consolidation work through the normal worker leases, cancellation, retries and
fenced transactions. It does not claim other queue items. Pause the installed
memory worker to prevent a competing provider from claiming the same queued work,
take an online SQLite backup, and use a separate configuration file pointing at
the intended database. Resume the worker and sync the Project's memory projection
afterward. No provider installation or daemon configuration change is required.

```sh
cargo run -p agentix-memory --example compact_codex -- \
  ONE_SHOT_CONFIG PROJECT_ID NEW_ARTIFACT_DIRECTORY --enqueue-legacy
```

`--enqueue-legacy` requires a complete Project page below one hundred records and
queues only active agent-authored records without a structured fact. Alternatively,
provide explicit work IDs after the artifact directory. The example checks their
Project and consolidation kind before execution. Artifacts contain memory evidence;
keep them with the migration's private diagnostic files. Luna reasoning is always
`low`. Request, task and lease budgets come from the supplied configuration, so a
bounded operational override can accommodate slower migration calls without changing
the production daemon's defaults. Successful writes still require normal projection
publication; this example never writes generated Markdown directly.
