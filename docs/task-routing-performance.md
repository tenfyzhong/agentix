# Task routing performance

Optional Jev routing moves candidate comparison outside the coding Agent's context.
It still adds a synchronous network request before the Agent starts. Local
measurements do not establish Jev latency, classification accuracy, or production
end-to-end improvement. Jev acceptance deliberately uses mock responses and does
not require live provider access or an API key.

## Hot path

- Disabled or incomplete configuration: the added prompt hook makes no CLI or HTTP
  call. Command-hook hosts still incur their normal Node startup overhead.
- Enabled: one `routing snapshot` process imports Inbox edits, renews leases, and
  returns bounded references, candidate summaries, and cancellation facts. Selecting
  a Job adds one `routing revision` process before delivering the route.
- Tool hooks with a valid turn receipt run one live heartbeat and omit repeated
  candidate discovery and workflow injection. Cancellation checks remain active.
- Prompt preparation and HTTP share an eight-second deadline. Transcripts are read
  with a 256 KiB limit. Candidate overflow or truncated requirements/waiting reasons defer to the Agent. Historical excerpts are marked separately, and terminal DONE/CANCELLED Tasks are excluded from the 256 unfinished-task budget.
  Fallback now uses a short delegation instruction and private snapshot reference.
  Its classifier returns at most 1,000 characters by protocol. Snapshot storage
  failure retains the previous 12,000-character fallback budget, excluding
  cancellation notices. The table below predates this delegation change.
- Candidate Jobs and Tasks use two SQL queries in one read transaction. SQL returns
  only six recent messages per Job; it does not iterate all history messages.
  Before HTTP, the host keeps two distinct historical messages per source, caps
  user/assistant excerpts at 512/256 UTF-8 bytes, removes redundant metadata and
  completed Task details, and enforces a 24,000-byte complete request limit for the
  32k context window. This is a conservative byte policy, not exact tokenization.
  SQLite still parses the stored JSON, so this is not constant-time in history size.

## Reproduce

Build the CLI from the repository root, then run in the plugin directory with
Node.js 24 or newer:

```sh
cargo build -p taskix
cd plugins/taskix-manager
TASKIX_ROUTING_BENCH=1 PATH="$PWD/../../target/debug:$PATH" node --test \
  --test-name-pattern='routing benchmark' tests/integration.mjs
```

The reusable tests create isolated databases, invoke real CLI subprocesses, mock
only HTTP, and assert process counts, route selection, and bounded injected context.
They report five warm-path samples per dataset. Fixture creation and initial Node
startup are excluded. These diagnostic timings are not CI thresholds.

A local debug-build run on 2026-09-21 produced:

| Candidate Jobs | History messages | CLI calls | Median local ms | CLI result bytes | Jev request bytes | Agent context characters |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | 0 | 1 | 16.0 | 435 | 954 | 731 |
| 1 | 0 | 2 | 24.7 | 910 | 1,216 | 1,028 |
| 8 | 0 | 2 | 22.8 | 2,884 | 3,057 | 1,028 |
| 32 | 0 | 2 | 22.4 | 9,674 | 9,391 | 1,028 |
| 33 | 0 | 1 | 17.5 | 9,482 | 0 | 6,741 |
| 1 | 10,000 | 2 | 32.2 | 1,868 | 1,608 | 1,420 |

The history fixture uses messages of approximately 110 characters. The 33-Job case
returns an incomplete snapshot and delegates without contacting Jev. Result bytes
are serialized CLI envelopes, summed across calls. Payload sizes depend on actual
content; the fixed budgets, rather than these sample sizes, govern larger inputs.

The earlier per-candidate process loop has been removed. Increasing candidate count
no longer starts another process per Job, and unselected candidates do not enlarge
the successful Agent context. Measurements do not currently justify a persistent
cache or another denormalized history table; both would add invalidation and
consistency costs. Larger real-world datasets can be evaluated with the same test
fixtures before changing that decision.

Compared with the pre-budget fixture run, the 32-Job HTTP request decreased from
16,007 to 9,391 bytes (41%), and the 10,000-message request from 2,466 to 1,608 bytes
(35%). Timing variation between runs is not evidence of a corresponding speedup.

## Delegated fallback

An earlier delegation benchmark of the same 33-Job fixture injected
944 characters instead of 6,741 (86% less hook context), with one CLI call
and no HTTP call. This historical measurement predates the additional validator
instructions and is not the current exact injection size. This excludes the classifier protocol read, spawn/wait tool
messages, returned decision, and any local reassessment. It is not an 86% reduction
in the whole Codex context or total billed tokens. The new work adds private local
snapshot I/O only on fallback; native subagent startup and inference add latency.

A native low-reasoning subagent with independent context was exercised on
2026-09-21 using `plugins/taskix-manager/tests/native-routing-smoke.mjs`. The
fixture created two competing Jobs in an isolated real Taskix database, simulated
a Jev outage, and passed the resulting private snapshot to the child. The child
selected the login fix Job without lifecycle writes. The parent validated the
compact result against live state, executed revision-guarded followup, and added
a Task while preserving the original dependency and required review policy.
This proves one semantic example and the local mutation chain, not classification
accuracy across a corpus or installed-client hook delivery.

The packaged parent validator refreshes assignment, Project, selected Job revision
and status, and selected Inbox availability. A changed revision rejects followup;
the parent must reassess instead of dropping the guard. Validation adds CLI calls
after child inference, so the earlier hook-only timings exclude this work.
See [integration coverage](integration-coverage.md#taskix-jev-routing) for the
native fixture procedure and default CI boundaries.

## Opt-in statistics overhead

`TASKIX_JEV_METRICS_ENABLED` defaults off. The disabled path does not load the
statistics module or SQLite and makes no statistics filesystem accesses. Enabling
it adds one local transaction per prompt in a separate database, executed in a worker with a 250 ms parent wait budget. The default-off path creates no worker. Statistics use
no extra CLI or model calls and are never injected into the Agent context. The
25 ms SQLite busy timeout limits lock waiting per operation; the worker deadline bounds parent waiting, including startup, rather than physical filesystem completion. Statistics writes are best-effort; use `taskix routing metrics report` to
inspect only successfully stored observations. Preparation durations exclude the
statistics write and any subsequent classifier subagent execution.


The default integration suite now measures the complete `runHook` call with real
CLI subprocesses and SQLite, including worker startup/persistence. HTTP inference
is mocked and initial Node startup and subsequent native classifier inference are
excluded. A local diagnostic run on 2026-09-21 measured 22.7 ms with statistics
disabled, 36.2 ms enabled, and 62.0 ms under a held metrics database write lock.
These are individual samples, not percentile guarantees or CI timing thresholds.
The report's `PREP_MS` column retains preparation-only semantics. Run the fixture
from `plugins/taskix-manager` with the compiled taskix on PATH:

```sh
node --test --test-name-pattern='real CLI prompt latency' tests/integration.mjs
```
