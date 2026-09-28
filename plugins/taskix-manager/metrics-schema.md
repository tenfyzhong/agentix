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
outcome, review_policy, or completion. Unrecognized kinds are stored as unknown.
One event covers one assessment, including preparation/network/revision fallbacks;
nested helper calls do not create duplicate events. Deterministic discussion
selection without a Jev evaluation is omitted. Disabled collection performs no I/O.
`action` is the classifier action, selected for accepted helper results, or agent; `reason` is the fallback code
or null. `answer_count` counts recorded expected questions, including invalid or
missing answers. Preparation/network failures can have zero recorded answers.

`answers.question` is intent, route, review_policy, inbox_N, turn_N, recovery, outcome, or completion. The review_policy
answer is recorded and gated only for work intent when the selected Job does not
already require review. Other intents and existing required Jobs do not need an
early policy classification. Lifecycle and discussion assessments are recorded under their own request kind. `subject_id` identifies the Inbox entry
for an Inbox question. `choice` contains only a known option (including selected
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
configured threshold; inbox_N and turn_N normalize to inbox and discussion.
A question may pass while its request falls back on another question or guard.
These are not independent human correctness labels. Existing JSON report fields
remain available. The default text summary fits 80 columns and 23 lines for all
supported kinds/questions; `--details` expands diagnostics and `--json` retains
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
