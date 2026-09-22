# Task routing performance

Optional Jev routing moves candidate comparison outside the coding Agent's context.
It still adds a synchronous network request before the Agent starts. Local
measurements do not establish Jev latency, classification accuracy, or production
end-to-end improvement. Deterministic acceptance tests use mock responses and do
not require live provider access or an API key. Opt-in historical replay uses the
real provider and reports acceptance separately from semantic accuracy.

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
  Fallback returns bounded summaries directly to the current main Agent, capped at
  12,000 UTF-16 code units excluding host discussion and cancellation notices. It does not create a delegation
  snapshot or request a classifier subagent.
- Candidate Jobs and Tasks use two SQL queries in one read transaction. SQL returns
  only six recent messages per Job; it does not iterate all history messages.
  The Job query also reads at most eight DONE Task summaries through the Job's
  task index, with titles capped at 300 characters before leaving SQLite. These
  summaries do not consume the 256 unfinished-task budget.
  For routing only, Codex/Claude read up to eight recent turns inside the existing
  256 KiB transcript limit; ordinary capture still reads the current turn. Pi/OMP
  retain eight recent visible messages in memory. If older transcript turns exceed
  the read limit, already parsed visible messages remain available even without
  the older turn boundary; incomplete JSON records are discarded. Unreadable current turns and cancellation still defer. Before HTTP, the host keeps up to
  eight distinct recent dialogue messages, capped at 1,024/1,536 UTF-8 bytes for
  user/assistant text and 8,192 serialized bytes overall. Long excerpts preserve
  both the beginning and the final decision. Candidate Job history uses the two
  latest eligible messages and, when absent from those, the latest eligible user
  message, capped at 512/256 bytes for user/assistant text. Older optional dialogue is removed if needed
  to preserve the complete candidate set and current prompt within the 30,000-byte
  request limit. The context ceiling is 32k tokens for state plus its longest
  question, matching the provider limit. The byte guard is
  deliberately conservative and is not an exact provider token count.
  SQLite still parses the stored JSON, so this is not constant-time in history size.

## References across turns

Jev receives the current assignment, an eligible `previous_job_id` hint, candidate
requirements and recent dialogue in chronological order. For an empty complete
candidate list, explicit candidate scope provides the zero count and eligible
ACTIVE/PENDING_REVIEW statuses. Continuing an untracked discussion
can require a new Job; completed Jobs cannot be reopened. An empty complete list
does not itself mean missing context. Jev still resolves the subject from the
dialogue and applies the normal confidence gates. A same-session boolean
adds provenance without sending session identities. The latest user message and
assistant response are also exposed as dialogue focus (4,096/8,192 UTF-8 bytes),
with all matching source Jobs. None of these facts selects ownership in code. The hint alone never
establishes ownership. Candidate `recent_conversation_indices` preserve exact
role/text matches to the final dialogue even when excerpts are shortened. This
retains the source of an assistant's proposed changes without repeating the full
text. All matching candidates remain visible; a text match never selects a Job
in code. Structured choice descriptions include Job titles and their dialogue evidence.
Two independent questions in the same HTTP request classify intent and ownership;
both must satisfy the existing confidence, probability and margin gates. Accepting a proposed change requests implementation; a question about
it remains discussion. An instruction such as "这样修改" can select the same Job's
`resume`/`followup` route; a question such as "有性能问题吗" combines
question intent with that Job owner. This returns `action: discussion` with `job_id` and the
selected Job context, after the same revision check. It does not reopen a pending
Job, acquire a Task, or write discussion ownership automatically. Related drafts
are attached through the existing guarded discussion workflow when work resumes.
Unrelated questions combine question intent with no candidate owner, and unresolved references
or conflicting assignments still defer to the main Agent.

This uses one existing Jev request, with no extra summarizer or keyword-based Job
selection. Related discussion adds a revision lookup just like other Job routes.
Larger bounded history and more route choices can increase request bytes and model
latency; deterministic tests establish transport and guard behavior, not improved
live acceptance or semantic accuracy. Thresholds and statistics isolation are unchanged.

## Historical replay result

On 2026-09-22, the final context implementation with a trial confidence threshold
of 0.50 accepted 712 of 838 historical requests (84.96%) across all 131 Agentix
Jobs present during the coverage audit. All fallback cases remain in the
denominator. The Job-level validation partition accepted 141 of 165 (85.45%).
This meets the 80% acceptance target; repeated context experiments did not reach
the 90% stretch target. Acceptance is not a labeled routing-accuracy measure.

The remaining cases were 101 uncertain/conflicting results, 24 incomplete
snapshots and one missing-context case. All three known requests spanning multiple
independent Jobs deferred to the main Agent. The largest serialized request was
29,731 bytes, within the conservative 30,000-byte guard for the 32k-token ceiling.
Historical reconstruction uses only evidence preceding each prompt and retains
incomplete cases; it cannot reconstruct every historical Inbox or lease state.
Credentials came from the user's interactive fish login environment. Replay did
not change Job/Task ownership. The default threshold was 0.90 during this
evaluation and was subsequently changed to 0.65. These results do not establish
deployed behavior or production latency.

For the threshold comparison and the recommended default of `0.65`,
see the [Jev confidence threshold guide](jev-confidence-thresholds.md). Its error
labels come from main-Agent review and remain subject to human confirmation.

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
returns an incomplete snapshot and falls back to the main Agent without contacting Jev. Result bytes
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

## Main-Agent fallback

Jev uncertainty, unavailable service and incomplete evidence return directly to the
current main Agent. The hook includes bounded candidate summaries and assignment
references. The Agent retrieves omitted facts before deciding ownership and reads
the selected Job revision before guarded followup. Conflicts require reassessment;
Taskix continues enforcing lifecycle, dependency and review rules.

This removes classifier subagent startup, inference, handoff and delegation snapshot
I/O. Candidate summaries now occupy the main Agent's context, within the fixed
12,000-code-unit budget. It does not establish a measured reduction in total tokens
or end-to-end latency; those depend on the host, conversation and model. Historical
hook-only measurements above exclude subsequent Agent reasoning and tool calls.
See [integration coverage](integration-coverage.md#taskix-jev-routing) for the real
CLI stale-write tests and live-host boundaries.

### Fallback rendering measurements

The 2026-09-22 compact-instruction regression measured the same small hook fixtures
before and after removing duplicated workflow prose:

| Visible candidates | Inbox entries | Before (code units) | After (code units) |
| --- | --- | --- | --- |
| 0 | 1 | 2,556 | 833 |
| 1 | 1 | 2,645 | 922 |

This is a 65–67% reduction in these fallback messages, not a whole-conversation
or billed-token saving. Both retain references and instructions to read omitted
facts before selecting task ownership. The fallback still supports up to 12,000
serialized UTF-16 code units; host discussion and cancellation notices are separate.

The pure renderer has no filesystem, CLI or network work. It reads only the first
32 entries of each candidate kind and serializes each bounded fragment once, so
rendering work does not grow with the remaining candidate bodies. A local Node
26.9.0 run of 1,000 iterations per fixture measured:

| Job candidates | Mean rendering ms | Output code units |
| --- | --- | --- |
| 0 | 0.00135 | 718 |
| 1 | 0.00065 | 806 |
| 32 | 0.00564 | 3,641 |
| 10,000 | 0.00637 | 3,645 |

These are diagnostic samples with already-materialized inputs. They exclude snapshot
loading, provider calls, host startup and Agent inference; they are not CI timing
thresholds. The low-count ordering reflects timer/JIT noise, not a meaningful speed
ranking. Reproduce from the repository root:

```sh
TASKIX_ROUTING_BENCH=1 node --test plugins/taskix-manager/tests/routing-context.test.mjs
node --test --test-name-pattern='compact_instructions' plugins/taskix-manager/tests/jev-runtime.test.mjs
```

## Opt-in statistics overhead

`TASKIX_JEV_METRICS_ENABLED` defaults off. The disabled path does not load the
statistics module or SQLite and makes no statistics filesystem accesses. Enabling
it adds one local transaction per prompt in a separate database, executed in a worker with a 250 ms parent wait budget. The default-off path creates no worker. Statistics use
no extra CLI or model calls and are never injected into the Agent context. The
25 ms SQLite busy timeout limits lock waiting per operation; the worker deadline bounds parent waiting, including startup, rather than physical filesystem completion. Statistics writes are best-effort; use `taskix routing metrics report` to
inspect only successfully stored observations. Preparation durations exclude the
statistics write and subsequent main-Agent handling.


The default integration suite now measures the complete `runHook` call with real
CLI subprocesses and SQLite, including worker startup/persistence. HTTP inference
is mocked and initial Node startup and subsequent Agent inference are
excluded. A local diagnostic run on 2026-09-21 measured 22.7 ms with statistics
disabled, 36.2 ms enabled, and 62.0 ms under a held metrics database write lock.
These are individual samples, not percentile guarantees or CI timing thresholds.
The report's `PREP_MS` column retains preparation-only semantics. Run the fixture
from `plugins/taskix-manager` with the compiled taskix on PATH:

```sh
node --test --test-name-pattern='real CLI prompt latency' tests/integration.mjs
```

## Historical replay evidence

The opt-in `tests/jev-live-replay.test.mjs` runner accepts a versioned JSON corpus
through `TASKIX_JEV_REPLAY_INPUT` and writes a checkpoint after every case to
`TASKIX_JEV_REPLAY_OUTPUT`. Run it with the configured login-shell environment.
It calls the real provider but performs no task lifecycle writes. Reports retain
all fallback cases, native confidence/probability, model version, latency, request
bytes and provider token usage. ACCEPTED measures routing coverage, not semantic
correctness; inspect accepted routes separately before treating them as accurate.

`tests/jev-history.mjs` can recover a missing request timestamp from a unique,
same-session `thread_goal_updated` creation event. It requires an exact objective,
zero usage, and an event timestamp within one second of creation; recurring goal
wrappers and later status updates cannot supply that timestamp. Only visible
messages before the creation event become history. Unrecoverable cases remain
in the replay denominator.
A unique image-caption match may also recover the timestamp when the original
user message contains an actual image part and a leading transport image wrapper.
Only that wrapper is removed for exact caption matching; literal markup without
an image, duplicate captions, and later replies cannot establish the timestamp.

`tests/jev-history.mjs` reconstructs candidate Job and Task snapshots from ordered
Taskix event exports. It uses UUIDv7 user-message timestamps instead of delayed
conversation capture times, excludes events in the message's second, and never
reactivates completed Jobs. Same-turn replies and other-session history are
excluded. Missing message timestamps remain incomplete cases in the denominator.
Historical Inbox, assignment leases and previous-job ranking are not reconstructed.
Later conversation attachments must not be used to inject future ownership labels.
For snapshot parity, `tests/jev-snapshot-replay.mjs` runs the checkout's actual
JOBS and TASKS SQL over the historical records in memory. This applies the same
field truncation, recent-message selection, terminal-task exclusion and overflow
flags as the CLI, without writing the live Taskix database. It requires a full
repository checkout. Unknown timestamps remain incomplete snapshots; no failed
case is removed from the denominator.
Candidate state also retains up to eight completed Task titles, each bounded to
384 UTF-8 bytes, with their DONE status. These describe delivered work when a
pending-review Job has no unfinished Tasks; they never become executable Tasks in
the selected context. Cancelled Tasks are excluded. Their bounded titles also appear
in the corresponding ownership criterion as delivered work, so approved scope
extensions are not hidden behind an older Job title or original requirement.
Reviews and fixes can refer to that delivered scope; shared technology alone
still does not establish ownership. A request spanning multiple independent Jobs
must defer to the main agent rather than select one partial owner. Candidate dialogue
retains its latest eligible user message alongside the two latest messages when
assistant progress updates would otherwise hide it. The extra message uses the
same byte-bounded excerpt and shared-dialogue deduplication; it adds no lookup.
The optional transcript reconstruction matches the exact request in the same
session, within ten seconds of a known message timestamp. Only a unique match is
accepted. It takes visible messages preceding that request, excluding subsequent
replies, reasoning, tool calls and host-injected context. Missing or ambiguous
matches retain their original cases; report transcript coverage separately from
Jev acceptance. This offline prefix reconstruction does not measure the live
transcript reader's byte-budget coverage.

`tests/jev-thresholds.mjs` projects acceptance at alternative thresholds from
recorded native answer scores without additional provider calls. It preserves
uncertain/invalid-answer rejection, the 0.2 probability margin, and non-score
fallbacks. The configured threshold applies to both native confidence and the
selected probability. Projections do not establish semantic accuracy or that live
assignment/revision guards pass; confirm a selected threshold with the normal
replay runner and inspect newly accepted decisions before changing configuration.

Keep counterfactual candidate stress tests separate from historical replay. A
corpus that changes completed Jobs to ACTIVE/PENDING_REVIEW is useful for candidate
competition, but cannot establish the production acceptance rate. Preserve its
results and rerun both comparison versions on identical historical inputs. Splits
must remain stable by Job; small tuning probes do not establish the target rate.
The optimization target is at least 80% ACCEPTED, with 90% as a stretch target;
Threshold sensitivity trials must be explicitly reported with their configured
threshold. Keep invalid/uncertain-answer, probability-margin, revision and snapshot
guards intact, retain failed cases, and do not infer accuracy from acceptance.
