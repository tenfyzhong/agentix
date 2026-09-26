# Jev lifecycle decisions

Jev classifies semantic intent. Taskix remains the authority for state transitions,
leases, Plans, dependencies, revisions and persistence. Enabling
`TASKIX_JEV_ENABLED=true` with valid URL/key configuration selects Jev first;
disabled, unavailable, uncertain or stale results use the existing main-agent
workflow. There is no additional classifier agent or automatic lifecycle writer.

## Shared architecture

`jev.mjs` shares the bounded state projection, conversation selection, HTTP
transport, confidence validation, deadline handling and revision check between
prompt routing and lifecycle assessment. Discussion classification reuses the same
transport. `lifecycle.mjs` obtains a routing snapshot and returns guarded command
arguments. Codex/Claude use its executable entry point; Pi/OMP call the same
function through their structured tool. Host adapters do not implement separate
classifiers. The packaged Skill defines when to call the helper.

| Decision | Jev responsibility | Deterministic responsibility |
| --- | --- | --- |
| New work / supplement | Ownership and scope; required/none review policy | Preserve existing required review; guard updates by Job revision |
| ACTIVE Job completion | Choose pending_review/completed from the whole delivered scope, replacing an earlier default if appropriate | Persist selected policy with revision guard before final Task transition; aggregate status |
| Job approve/reject/cancel | Explicit user acceptance, rejection or abandonment | Valid source state, revision and actual transition |
| Task recovery | Resolved waiting/blocking reason; retry/reopen/cancel/release intent; unambiguous target | Ownership, Task revision, dependency and lease checks |
| Task outcome | Continue/wait/block/fail/readiness from visible evidence | Valid execution state; verify real acceptance evidence before done |

Readiness is not `task done` or Job approval. It returns no write arguments.
Approval requires explicit user acceptance of the delivery, not permission to
start implementing a plan. Existing required review cannot be downgraded by a
Git-only supplement. Release/tag/PR operations count as operational work; they
are distinct from Taskix Job/Task state commands.

The existing state fields and excerpt bounds are reused. A specifically named
terminal Task can be read for reopen assessment; this adds only that Task using
the same field whitelist and SQL-equivalent size limits, not the terminal history.
No lease token or credentials enter the model state. Partial snapshots, oversized
requests and conflicting ownership defer. The 0.65 default confidence threshold,
0.2 runner-up margin, 30,000-byte request limit and 8-second deadline are unchanged.

## Performance review

Routing intent, ownership, Inbox matching and review policy share one HTTP call.
A lifecycle checkpoint uses one HTTP call; the completion checkpoint is called before the final Task transition, after actual acceptance verification. It returns a policy update, not Job approval. Direct standalone CLI calls continue to use their saved policy. Disabled assessment performs no CLI or
network I/O. Snapshot and revision reads are bounded, with one optional read for
an explicitly named omitted terminal Task. Requests do not fetch complete history.
The helper's deadline covers its snapshot and optional Task read as well as model
inference and revision validation. No retry loop or result cache can reuse a stale
lifecycle decision. Repeated route instructions are emitted once rather than once
per candidate, retaining capacity for 32 candidates.

Local deterministic routing benchmark on 2026-09-26 (real CLI and SQLite, mock
provider; medians of the existing benchmark's iterations):

| Eligible Jobs | Conversation messages | Median ms | CLI processes | Request bytes |
| --- | --- | --- | --- | --- |
| 0 | 0 | 16.8 | 1 | 5,465 |
| 1 | 0 | 23.2 | 2 | 5,436 |
| 8 | 0 | 24.2 | 2 | 8,425 |
| 32 | 0 | 29.1 | 2 | 18,717 |
| 33 | 0 | 18.2 | 1 | 0 (bounded fallback) |
| 1 | 10,000 | 32.7 | 2 | 6,220 |

These measurements isolate local preparation overhead; they are not production
network latency or a proof of a globally optimal architecture.

## Historical replay and tuning

The source was a read-only export of the Agentix Project's 141 Jobs and 497 Tasks,
whose Obsidian projections are under `11-Agents/Projects/agentix`. The selected
21 source Job IDs/names and the selected Task ID were verified against those
notes. Existing historical records and notes were not changed by replay.

The tuning set contains 31 manually labeled cases: 12 original historical initial
requests, three original PR follow-up requests with preceding conversation and
reconstructed pending-review state, and 16 synthetic boundary/recovery/outcome
requests using real Job/Task facts with explicitly reconstructed state. The
production SQL projection constructs the candidate snapshots. No future
conversation is supplied. Empty-candidate initial cases test scope classification,
not an exact reconstruction of all concurrent Jobs at the historical instant.

The held-out set contains six additional original historical requests with
manually labeled scope and an empty eligible candidate list. It was run after
tuning and was not used to change the final instructions. These are small,
selected samples, not a representative accuracy benchmark.

| Run, Jev response model `jev-1.13.0` | Cases | Accepted and matching labels | Wrong accepted | Deferred | Mean / p95 ms | Largest request |
| --- | --- | --- | --- | --- | --- | --- |
| Before instruction tuning | 31 | 21 | 0 | 10 | 531 / 1,210 | 12,835 bytes |
| After instruction tuning | 31 | 26 | 0 | 5 | 474 / 667 | 13,135 bytes |
| Held-out original requests | 6 | 3 | 0 | 3 | 587 / 1,059 | 5,825 bytes |

The change clarified overlapping review-policy criteria: operational Git delivery
was being confused with a Taskix state operation. Thresholds, context and labels
were not weakened. Some indirect requests still defer because intent, ownership
or policy lacks confidence. The held-out feature request about enabling Task
cancellation also deferred; this remains a model distinction to monitor. A bare
completion claim without acceptance evidence deferred as intended. Zero observed
wrong accepted decisions does not imply zero future errors. Repeated service
calls can vary, so the latency difference is not attributed to the wording change.

Run the reusable replay harness with explicitly reviewed private fixtures:

```sh
TASKIX_JEV_LIFECYCLE_REPLAY_INPUT=/path/to/cases.json \
TASKIX_JEV_LIFECYCLE_REPLAY_OUTPUT=/path/to/results.json \
node --test plugins/taskix-manager/tests/jev-lifecycle-live.test.mjs
```

The fixture has `schema_version: 1`, a `method` description, and `cases`. Each case
contains `id`, `source_job_id`, `provenance`, `prompt`, bounded `context`, `expected`,
and optional `history`, `session_ref`, and `assessment` (`kind`, `job_id`, optional
`task_id`). `tests/jev-snapshot-replay.mjs` exports `boundedSnapshot` for the
production SQL projection. The harness calls the production classifier and only
permits an in-memory revision read; it cannot execute lifecycle writes. It saves
raw answer scores and incremental summaries to a private output file. Preserve
the original results when tuning; report abstention separately from wrong accepts.
Private conversation fixtures and credentials are not committed.

## Completion checkpoint replay

After adding the final Job destination assessment, six historical Job scopes were
replayed with ACTIVE status and an initial required policy explicitly reconstructed.
The original requests and real Task titles used the same production SQL projection.
Jev accepted three release/tag Jobs as completed and one implementation Job as
pending_review; two implementation Jobs deferred and retained required review.
All four accepted results matched manually reviewed labels (no wrong accepts).
Mean latency was 516 ms, p95 980 ms, and the largest request was 3,456 bytes.
This is an additional small checkpoint sample, not a general accuracy estimate.

## Verification

The full plugin suite passed 432 tests with seven opt-in skips. Real CLI
integration passed 70 tests with six benchmark skips; those six benchmarks passed
separately. The native metrics compatibility test also passed separately. The
live lifecycle replay and held-out replay passed with the outcomes above. The
remaining optional skips cover the older live routing replay, native OMP install,
Homebrew bottle/tap checks and a fallback-rendering benchmark; they are not new
lifecycle coverage. The existing Rust plugin-entrypoint integration also passed.

Deterministic tests cover enabled/disabled operation, each semantic action,
review-policy preservation and upgrade, all four host paths, mocked HTTP errors,
ambiguous/invalid answers, stale revisions and real CLI transitions. The real CLI
policy matrix verifies that the saved policy controls `PENDING_REVIEW` versus
`COMPLETED`. Readiness leaves both Task and Job unchanged. Tests do not establish
that a live model is always correct or that every possible user utterance is
covered. See [integration coverage](integration-coverage.md#jev-lifecycle-and-review-policy)
and [command reference](../plugins/taskix-manager/skills/taskix-manager/references/commands.md).
