# Jev metrics storage protocol

The plugin owns event collection, empty-database initialization and append-only
request/answer writes. Taskix owns reports, threshold comparisons and manual review
updates. The plugin has no report/list/label API or command-line interface.
Neither component uses the task database for these observations.

## Identity and compatibility

The canonical v2 DDL is [metrics-schema.sql](metrics-schema.sql), packaged with the
writer. SQLite `application_id` is `0x544A4556` (TJEV); `user_version` is `2`.
The CLI reads v1 and v2; the writer upgrades a recognized v1 database to v2. The version is a database-wide
storage contract, distinct from taskix's JSON response `schema_version`.

The plugin may initialize only an empty database with application_id=0,
user_version=0, and no user-defined schema objects. Initialization, metadata and
the first event commit together under BEGIN IMMEDIATE. Existing supported databases
receive inserts with named columns. The v1-to-v2 migration adds
`requests.kind TEXT NOT NULL DEFAULT 'routing'` and sets user_version=2 in the
same transaction as the new event. Old observations are routing-only and retain
their labels and answers. A failed event rolls back the migration too. Reports
and labels can use v1 without migrating it; upgrade CLI and plugins together
because older readers reject v2. Other schemas are never repaired implicitly.
Unsupported versions, foreign databases, and nonempty unversioned databases remain
untouched. The plugin warns and skips the sample; taskix returns an explicit error
before querying or labeling. A failed event rolls back both request and answers.

Unversioned databases produced by the earlier development implementation are not
automatically adopted. Preserve them and select a new TASKIX_JEV_METRICS_DB path
for v2 collection. Do not change PRAGMA metadata manually to disguise another
schema as a supported version. Future incompatible changes require a version bump, a documented
migration path and compatibility tests; do not repurpose existing fields.

## Records and semantics

`requests.id` is a generated UUID and joins `answers.request_id`. `started_at` is
Unix epoch milliseconds. `duration_ms` is monotonic preparation elapsed time,
excluding metrics persistence and subsequent Agent handling. Session/turn and
Project references are nullable when unavailable. Model and configured threshold
are captured per request. `called` and `accepted` are integer booleans; accepted
means the host accepted Jev's decision after applicable guards, not that a human
confirmed it was correct. `kind` is routing, discussion (turn ownership), recovery,
outcome, review_policy, completion, or memory_triage (extraction screening). Kinds are open-ended stable identifiers matching `[a-z][a-z0-9_]{0,63}`; invalid identifiers are rejected before storage is opened. New kinds need no whitelist or report registration.
One event covers one assessment, including preparation/network/revision fallbacks;
nested helper calls do not create duplicate events. Deterministic discussion
selection without a Jev evaluation is omitted. Disabled collection performs no I/O.
`action` is the classifier action, selected for accepted helper results, or agent; `reason` is the fallback code
or null. `answer_count` counts recorded expected questions, including invalid or
missing answers. Preparation/network failures can have zero recorded answers.

`answers.question` is intent, route, review_policy, issue, turn_N, recovery, outcome, or completion. The review_policy
answer is recorded and gated only for work intent when the selected Job does not
already require review. Other intents and existing required Jobs do not need an
early policy classification. Lifecycle and discussion assessments are recorded under their own request kind. `subject_id` identifies the Issue entry
for an Issue question. `choice` contains only a known option (including selected
Job identity for followup/resume) or null. Confidence, selected probability and
runner-up margin are nullable for invalid answers. `valid` is a boolean independent
of the configured confidence gate. `issue` is null, invalid_answer,
uncertain_choice, low_confidence or small_margin. An entire prompt passes only
when every applicable expected answer passes; later ownership/revision checks can still defer.

`requests.review` is null until a human labels the complete proposed decision correct/incorrect. Only taskix modifies this field; later plugin
appends preserve labels. Reports distinguish adoption from reviewed accuracy and
score-gate counts from predicted adoption. `by_kind` aggregates request counts,
called/accepted counts, adoption, reviewed accuracy and preparation duration.
`by_question` aggregates answer counts and score-gate passes using each request's
configured threshold; turn_N normalizes to discussion; issue is already a single-choice question.
A question may pass while its request falls back on another question or guard.
These are not independent human correctness labels. Existing JSON report fields
remain available. The default text summary fits 80 columns and 24 lines for the seven
built-in kinds/questions; additional kinds add rows. `--details` expands diagnostics and `--json` retains
all data. Historical v1 data cannot reconstruct past unrecorded lifecycle calls. No prompt/history bodies, credentials,
endpoint URLs, raw model responses or task leases belong in this schema.

## Verification

Rust CLI tests cover report/list/label semantics and incompatible-version refusal.
Node tests cover writer initialization, disabled/no-I/O behavior, incompatible
metadata, atomic writes and preservation of existing data. The interop test writes
via the real plugin, reads and labels via the native CLI, then appends again to
verify labels survive. The default `cargo test -p taskix --all-features` integration entry supplies the compiled binary and runs this test in CI, alongside concurrent-process and whole-hook timing tests. Standalone Node runs may omit this one native-CLI test when no binary is supplied. To run it directly after building taskix:

```sh
TASKIX_TEST_METRICS_BIN="$PWD/target/debug/taskix" node --test \
  plugins/taskix-manager/tests/jev-metrics.test.mjs
```


The writer runs in an isolated worker; the parent waits up to 250 ms, then
requests termination without waiting for a blocked OS operation. SQLite retains
its 25 ms per-operation busy timeout. Termination or contention may drop a sample;
a committed request must always have its complete answer set. These limits do
not redefine `duration_ms`: the CLI displays it as `PREP_MS`, not end-to-end time.

The memory service writes `memory_triage` using the same v2 schema and environment
variables. `action` is `skip`, `extract`, or `agent` (fallback). The question is
`memory_triage`; its `subject_id` identifies source receipt and work item. Confident
skip and extract results have `accepted=1`; uncertain, low-score, malformed and
service-failure results produce `agent` (fallback). Answers preserve Jev's
original choice, scores and validation issue. A temporary positive-gate experiment
used the same kind but was rolled back; compare historical metrics within their
policy/time window. These are screening policy
outcomes, not completed work or model-call savings. Retries/chunks are separate
evaluations. Disabled Jev does not emit requests. The compact report adds one
skip/extract/fallback summary when such records exist. The Rust writer performs
the same transactional v1 migration; a failed append rolls back migration too.


## Runtime coverage

| Request kind | Jev decision | Collector |
| --- | --- | --- |
| `routing` | Prompt intent, Job ownership, applicable review policy and Issue association | Shared host runtime and `routePrompt` |
| `discussion` | Ownership of pending conversation turns | `selectDiscussion` |
| `recovery` | Task recovery or explicit Task transition | `assessLifecycle` |
| `outcome` | Continue, wait, block, fail or ready assessment | `assessLifecycle` |
| `review_policy` | Scope-based review requirement | `assessLifecycle` |
| `completion` | Whole-Job completion destination | `assessLifecycle` |
| `memory_triage` | Skip or retain a chunk for memory extraction | Memory service `JevTriage` |

All four agent hosts use the shared plugin collectors. Issue matching is an
`issue` question within a routing request, not a separate provider call.
The report groups recorded kinds dynamically; a missing row means no retained
sample of that kind, not that the kind is unsupported. Enable both
`TASKIX_JEV_ENABLED` and `TASKIX_JEV_METRICS_ENABLED` in each calling process,
and use the same `TASKIX_JEV_METRICS_DB` to combine host and memory-service data.
Disabled or invalid Jev configuration produces no evaluation record.
Deterministic discussion selection without a provider call is also omitted.

Offline replay tests and `scripts/taskix-memory-eval.py` keep their own evaluation
results and do not append experiments to runtime metrics. Memory extraction and
consolidation use their configured model provider, not Jev, and are outside this
report. Runtime provider failures and preflight fallbacks are recorded when
collection is enabled, subject to the bounded best-effort writer.

The combined CLI regression covers all seven kinds and their compact/JSON views.
A mocked HTTP triage regression verifies one persisted event per actual decision,
including skip, extract, uncertainty, malformed answers, service errors and
oversized input, with no source text or credentials in the metrics database.


## Adding a Jev decision

Collection, persistence and presentation have separate responsibilities:

- `jev-metrics.mjs` provides `measureDecision` and bounded worker persistence for JavaScript. `withJevMetrics` in `jev.mjs` adapts the configured Jev provider to that collector.
- `crates/taskix/src/metrics/collector.rs` provides typed `DecisionMetric` / `AnswerMetric` events and `MetricsCollector` for Rust callers.
- `metrics.rs` dispatches CLI commands; `metrics/report.rs` aggregates and renders recorded kinds/questions dynamically.

Choose a constant `snake_case` kind and stable question identifiers. Do not put
user text, provider errors, prompts or credentials in kind/action/reason fields.
Callers own provider invocation, answer validation and the final guarded decision;
collectors own storage, atomicity and the 250 ms persistence budget. Both writers
use the existing v2 schema and support multiple answers per decision.

A JavaScript caller can use:

```js
return withJevMetrics(env, options, async metric => {
    if (metric) metric.called = true;
    const decision = await evaluateAndValidate();
    if (metric) {
        metric.answers = decision.scores; // answerScore() results, not raw replies
        metric.outcome = decision.accepted
            ? { action: decision.action }
            : { action: "agent", reason: decision.reason };
    }
    return decision;
}, "risk_review");
```

The collector records timing in `finally` and preserves the callback result or
exception. Handle provider failures in the decision adapter and set a sanitized
fallback reason before returning/throwing. Nested adapters must pass the existing
telemetry to `measureDecision` instead of opening another measurement scope.

A Rust caller creates `DecisionMetric::new(kind, model, threshold)`, fills the
optional references, `called`, final `action`/`reason` and typed `answers`,
and sets `duration_ms` from its preparation/provider timer. Obtain an optional
`MetricsCollector::from_env()` once, then await `collector.record(&event)`.
Its boolean result describes metrics persistence only; ignore a failed write when
returning the original domain decision. `agent` is the common fallback action;
a called, guarded non-fallback decision counts as accepted.

No CLI dispatch, SQL schema or known-kind list needs changing for a new kind.
Add a collector regression and use the native CLI interoperability test to verify
that the new kind/question appears in `by_kind`/`by_question`. Specialized legacy
summaries such as memory skip/extract remain presentation conveniences; new kinds
automatically receive the standard statistics.
