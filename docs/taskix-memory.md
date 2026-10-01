# Taskix project memory

Taskix remembers reusable project decisions, reasons and rejected alternatives,
constraints, external facts and verified lessons that cannot be found in the
repository. It keeps evidence and revisions, distinguishes user decisions from
assertions, observations and inferences, and does not treat brainstorming as a
confirmed decision. Repository facts, progress reports and internal tool chatter
are not useful memory candidates.

Supported historical experiences and events with lasting significance can be
external facts. Keep their source attribution and date, and distinguish them
from claims about current status. A dated historical assertion does not expire
merely because the event ended; ongoing external conditions may still need expiry.

Memory maintenance runs in **`taskix memory serve`**, a separate process shipped
in the same executable and release as the task board. It uses the same Taskix
configuration file. The service is available on macOS, Linux and Windows. This delivery does not install or launch a
background service automatically.

## Enable and operate

Initialize Taskix and register the project normally. Add these sections to the
existing `~/.config/taskix/config.toml` (or the file selected by `TASKIX_CONFIG`
or `--config`):

```toml
[memory.providers.openai]
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[memory.agent]
provider = "openai"
api = "responses"
model = "gpt-6-astra"
# Optional: omit to use the provider default.
reasoning_effort = "low"
```

`memory.agent.reasoning_effort` accepts `none`, `minimal`, `low`, `medium`,
`high` and `xhigh`. The selected model/provider must support the requested level;
unsupported levels produce a provider error without silently changing the model
or effort. Omission sends no effort setting. Responses receives `reasoning.effort`;
Chat Completions receives `reasoning_effort`. This setting applies to extraction,
consolidation, repository review and deep queries. `taskix memory reload` applies
changes to new work; running loops retain their configuration snapshot.

Export `TASKIX_MEMORY_ENABLED=true` or `1` to the CLI, agent host and memory
service. Unset or any other value disables memory. Hooks read this variable
directly and skip memory CLI calls when disabled; the CLI uses the same rule.
Provider, embedding, projection and retrieval settings remain in `config.toml`.

Provide the named provider credential to the **service process**, then run:

```sh
export TASKIX_MEMORY_ENABLED=true
taskix memory doctor
taskix memory serve
# From another terminal in a registered project:
taskix memory status
taskix memory search 'offline deployment'
```

Use a process manager such as launchd or a systemd user unit to run the same
foreground command if desired. Give it an absolute executable/config path and
configure the required provider credentials. On macOS/Linux, `memory serve`
loads a complete exported environment snapshot from the user's account login
shell using `-lc`, following Agentix's Codex startup approach. Override the shell
with `TASKIX_LOGIN_SHELL`, an absolute executable path supplied to the service.
The lookup has a three-second deadline. Failure, missing/malformed framing or a
timeout emits a diagnostic and retains the inherited environment; environment
values and shell output are not logged. Successful snapshots honor shell `unset`
operations and preserve whitespace and non-UTF-8 environment bytes.

The service then uses `exec` to restart itself once with the snapshot before
loading configuration or initializing providers. The executable, arguments,
working directory, PID and standard streams are preserved, so launchd/systemd
continue supervising the same process. No global process environment is mutated.
Only `memory serve` performs this lookup; ordinary commands, diagnostics and
configuration reload do not. Windows uses the environment supplied by its
launcher and does not invoke a Unix login shell.

If Fish is your account login shell, export `TASKIX_MEMORY_ENABLED`, service API
keys, proxy settings and Jev variables in its configuration with `set -gx`, outside `status is-interactive`
guards. Variables set only in an existing terminal are not available to a new
login shell started by a service manager. The main agent's subscription is not
provider authentication. Homebrew formulae can invoke `taskix memory serve`
directly without a Fish dependency or a shell wrapper.

Ctrl-C stops service background loops; Unix
also accepts SIGTERM and Windows accepts Ctrl-Break.
Unfinished work resumes after its durable lease expires. One service can manage
many Projects; a second process for the same database is rejected.

The default memory path is `memory.sqlite3` beside `[storage].path`. Override
with `[memory.storage].path`, using a distinct absolute path (`~` is expanded).
Keep this database: human edits, lifecycle decisions and suppression markers
cannot be regenerated from conversation history. Indexes and vectors are derived.

The complete [configuration example](../config/taskix.example.toml) includes all
limits. Ordinary board operations ignore memory model configuration. Memory
reads load only location/retrieval settings and remain available when model
configuration is invalid or the vault is missing. Missing provider credentials
leave the service available for FTS and manual edits; maintenance reports its
provider error. Invalid full service configuration prevents startup or reload.

`taskix memory reload` applies a validated snapshot to future work. In-flight
loops finish with their original model/settings. Query admission limits retain
process-wide accounting across reloads. Named providers also share admission across old and new model instances; changing
`max_in_flight` updates this shared budget. Lowering it lets active requests
finish and withholds new requests until usage falls below the new limit.
Database paths, enabled state and IPC connection/size limits require a restart; an incompatible reload keeps
the running configuration intact. `memory doctor` checks configuration,
credential availability, database identity and service availability without a
model request. Credential availability in doctor refers to the CLI environment;
`service.provider_errors` describes the running process. `doctor` and `status`
also report an uninitialized database when its parent directory is missing,
without creating that directory.

## Optional Jev extraction screening

The memory service reuses the existing Jev connection, enable and metrics
environment variables, with an independent memory confidence threshold. There is no
separate memory triage configuration section. Supply these to the **service
process**, just as for the host plugin:

| Variable | Behavior |
| --- | --- |
| `TASKIX_JEV_ENABLED` | `true` or `1` enables screening, case-insensitive; otherwise extraction goes directly to the model |
| `TASKIX_JEV_URL` | Existing Jev evaluation URL, used directly |
| `TASKIX_JEV_API_KEY` | Existing Bearer credential |
| `TASKIX_JEV_MODEL` | Defaults to `jev-latest` |
| `TASKIX_MEMORY_JEV_MIN_CONFIDENCE` | Memory-only threshold, defaults to `0.75`, range `0.5..1`; never reads or falls back to `TASKIX_JEV_MIN_CONFIDENCE` |
| `TASKIX_JEV_METRICS_ENABLED` | `true` or `1` enables best-effort metrics |
| `TASKIX_JEV_METRICS_DB` | Existing metrics database override; otherwise the usual XDG state path |

Missing or invalid Jev settings bypass screening without blocking extraction.
Changing a service process's environment requires restarting it. On macOS/Linux,
the restart reads a fresh login-shell snapshot; `memory reload` updates only the
configuration. Changing a shell or the main agent's environment does not update
an already running service.

Each extraction work item is screened before repository tools or the extraction
model run. Jev receives the full source snapshot and the current chunk; source
text is treated as data. Only a valid `skip` with confidence and selected
probability at least the configured threshold and a margin of at least `0.2`
suppresses the model call. `extract` continues normally. Uncertain, malformed,
low-confidence or small-margin answers, HTTP errors and the eight-second deadline
all use model extraction. Requests
above 30,000 UTF-8 bytes bypass Jev without truncating decision context; responses
are capped at 1 MiB. Consolidation, repository reviews and queries are not gated.
Screening concurrency is bounded by the existing worker pool.

The screening question distinguishes memory value from task progress. Pure CI
updates, delivery receipts, routine operations, injected execution boilerplate,
raw tool output and repository-visible implementation descriptions may be skipped.
Substantive choices, rationale, external limits, incidents and measurements remain
eligible even inside progress updates. The current chunk is the decision target;
other source messages only resolve context. Unresolved approvals or missing
incident evidence remain eligible for conservative extraction.

Skipping finishes that extraction item with zero candidates. Its source remains
stored, and its decision is visible in the work audit. Lease and source-revision
checks prevent an old screening result from completing superseded work. This
classifier can still make semantic mistakes; score thresholds are not an
accuracy guarantee. No observation-only mode is enabled implicitly.

`taskix routing metrics report` adds the `memory_triage` request and question
category, plus counts of skip, extract and fallback decisions. `--details`,
`--json` and `routing metrics list` retain confidence, score gates and fallback
reasons. Counts are per work-item evaluation, including retries and chunks, not
unique turns. `accepted` means the classifier result passed validation; both
skip and extract can be accepted. It does not mean a memory was created or that
a fenced work completion succeeded. Disabled/misconfigured Jev produces no Jev
metric, matching host behavior. Metrics failures never change extraction;
metrics contain identifiers and scores, not conversation text or credentials.

## Models and embeddings

Extraction, consolidation and repository review share `[memory.agent]`; each
work item and retry starts a fresh Agent context. Clients and connection pools
are reused, conversations are not. Models are called through explicit APIs,
with structured tool responses and application-side evidence validation.
The loop can page through preceding turns in the same Project and session, then
read original messages to resolve references such as "use the second option."
Each task still starts with fresh model context; history is retrieved on demand.
Turn ordering and current source metadata are indexed, including a transactional
backfill when opening an older database. Neighbor lookup returns summaries;
message bodies are read separately when needed.

`[memory.agent].extraction_debounce_ms` defaults to `1000` (range `0..60000`);
zero disables the wait. Claimed extraction waits before Jev or model execution
so rapid revisions can supersede it without paying for a request. Every source
snapshot remains durable. Lease validity is checked approximately every 200 ms;
superseded or expired work drops its running model/tool future. Provider-side
billing may already have occurred. An unchanged message is not treated as a
permanent cache hit: new context can require a new decision. The lease must
exceed the task timeout plus the debounce rounded up to seconds.
`api = "responses"` and `api = "chat_completions"` are supported for compatible
models/providers. GPT-6 Astra requires Responses. There is no automatic model
fallback or login-session subprocess.

Embedding is optional. FTS5 supports Chinese segmentation and English identifiers
without network calls. Enable semantic recall by adding:

```toml
[memory.embedding]
enabled = true
provider = "openai"
model = "text-embedding-3-small"
# dimensions = 1536
batch_size = 16
max_concurrent_projects = 4
request_timeout_seconds = 10
```

For a separately running Ollama embedding endpoint:

```toml
[memory.providers.local_embedding]
protocol = "ollama"
base_url = "http://127.0.0.1:11434"
max_in_flight = 2

[memory.embedding]
enabled = true
provider = "local_embedding"
model = "YOUR_INSTALLED_EMBEDDING_MODEL"
# Set query_prefix/document_prefix if that model requires retrieval prefixes.
```

Replace the existing embedding section; TOML cannot declare it twice. The
service does not download Ollama models. Omit `api_key_env` for an unauthenticated
local provider. OpenAI-compatible embeddings use `/embeddings`; Ollama uses
`/api/embed` with truncation disabled. Dimension zero is never an embedding:
omitted dimensions are discovered from the first successful response.

Model, endpoint, dimensions or prefix changes create a new index generation.
Asynchronous results are fenced by generation and memory revision; mixed vectors
are never searched together. Background batches fill the new generation while
FTS stays usable. Search combines independent lexical and exact cosine rankings
with reciprocal rank fusion. Responses report `fts`, `hybrid`, `fts_fallback`,
or `offline_fts`. `status --project PROJECT_ID` reports the active profile and
indexed count. `reindex` rebuilds FTS in bounded pages; changing the embedding
profile triggers its own rebuild. Old vector generations are not used for recall.

Background embedding runs at most one batch per Project and up to
`max_concurrent_projects` Projects concurrently (default `4`, range `1..32`).
Admission rotates fairly and completed Projects cool down independently, so
one slow Project does not hold an entire round. Provider `max_in_flight` still
applies to all requests together. With a provider limit of two or more, background
model/embedding work leaves one slot for online query embeddings. With a limit of
one, active requests are not preempted; a queued query precedes subsequent
background batches. Reserving capacity reduces peak background throughput.
Reload cancels old batches; generation/revision fences remain active.
The Project list refreshes every five seconds instead of on each completion.
Empty Projects back off exponentially to a maximum of 30 seconds. Committed
memory changes in this service wake their Project immediately; changes from
another process are discovered by periodic scans. Lost notifications trigger a
recovery scan. Nonempty batches and errors retain the normal polling interval,
and per-record failure backoff still applies.

Embedding failures are persisted per memory revision and index generation, with
backoff and at most three automatic attempts. Input-related HTTP 400/413/422
batch failures are retried individually so one rejected document cannot block
later valid records. Outages, authentication errors and rate limits do not fan
out into individual requests. `status` includes up to 20 current
`embedding_failures` with attempts, retry time and error. `memory reindex` clears
failure state for the records on its page, permitting an explicit retry; it does
not discard already valid vectors. A memory edit or new model generation also
permits fresh attempts. Responses incompatible with the current profile's resolved
dimensions also enter failure backoff; results from superseded generations are
simply discarded. Retry state survives daemon restarts and paired backups.

Query vectors have a 64-entry, one-minute, project/generation-scoped cache;
up to 64 distinct in-flight queries are shared by Project, generation and exact
query text. Each caller keeps its own wait deadline. One caller timing out does
not cancel another caller's request; shared work has the configured query timeout.
Failures are not cached. Dropping the old runtime cache cancels its pending work.
Retrieved memory results themselves are not cached, so forgetting and revisions
take effect on subsequent reads. Provider failures/timeouts fall back to FTS. Exact vector
scanning uses 256-row pages and a fixed-capacity top-k heap, sorted once at the
end with deterministic score/ID ordering. Its CPU/I/O cost still
grows with the number and dimensions of vectors in the selected Project.

## Source delivery, concurrency and cost

Visible turns are captured transactionally in the task database, together with
an immutable outbox receipt. Explicit Project/Job ownership and canonical cwd
resolve scope; unknown ownership is not guessed from the latest Job. Worktrees
share their canonical Project. Pure discussions can be sources without a Job.
Late replies and corrections create new source revisions. Deleted/expired
original discussion records do not remove already retained evidence. Project
archival stops new background maintenance; history remains explicitly readable.
Missing registered workspace directories are treated as unavailable repositories.
Periodic repository reviews skip them and resume when the directory returns;
existing memories and evidence remain readable. Extraction that needs an unavailable
repository fails through the normal work retry mechanism rather than accepting
unverified candidates. After the retry budget is exhausted, restore the directory
and use `taskix memory retry WORK_ID`. Permission errors and roots replaced by files
still report background errors.

`background_errors.worker` records the most recent worker failure until a subsequent
work item succeeds; empty queue probes do not clear it. Individual work errors and
retry states remain available through `taskix memory work WORK_ID` even after the
worker resumes successful processing.

The service commits receipt, evidence and queue entries before acknowledging the
outbox. Acknowledgement means **received**, not extracted. No model work runs
inside the main agent or task transaction. Intake failures are retried separately
from the forward replay scan. Replay has its own durable checkpoint, advanced
only after each input has been persisted. It stops at a failed input and retries
without skipping it, even when later inputs arrived through pending intake.
The maximum stored source sequence is not a recovery checkpoint. Databases from
before checkpoint support perform one idempotent replay from zero to repair gaps.
Durable retries, leases and source revisions fence stale workers. Startup checks
stored sources against the task history and resumes ordered replay, including
acknowledged inputs after an older memory restore. Source snapshots are
deliberately retained; there is no automatic prune.

The default worker budget is four concurrent loops, at most two extraction loops
per Project, and one consolidation/review loop per Project. Different Projects
can consolidate concurrently; fair scheduling prevents a single busy Project
from owning all claims. Live intake precedes historical backfill and review
within a Project. Providers also have an independent in-flight limit. Normal
queries and optional deep queries have separate service limits.

Sources are split at UTF-8 boundaries into 16 KiB chunks. Candidate consolidation
is split into bounded batches; each atomic memory document is limited to 60 KiB.
Default loop budgets are 12 steps, 24 tool calls, 128 KiB serialized context,
8,192 output tokens, 90 seconds per request and 240 seconds per work item, with
three attempts. Invalid `repo_search` arguments return corrective feedback to the
model within the same loop and consume the normal call budget. They do not count
as repository inspection. Storage and other operational failures still fail the
work attempt. Empty model replies receive a reminder to submit through a tool.
Extraction and consolidation submissions receive pure structural validation before
acceptance; invalid fields or missing current-source evidence return bounded
feedback in the same loop. These corrections consume the original step, call,
context and timeout budgets. Database fencing and literal-source validation still
run atomically at commit and are not bypassed. Each memory permits one to sixteen
evidence quotations; an oversized merge should become a separate scoped memory
instead of discarding prior evidence. Empty extraction workers back off up to five seconds, with
post-commit notifications for intake, consolidation work, scheduled reviews and
explicit retries. A work-generation check prevents a stale empty result from
overriding a newer notification. Periodic probes recover cross-process writes
and expired leases; existing retry availability and concurrency gates still apply.
Lowering the context limit below a work item's needs causes a
reported failure rather than truncating evidence. `work` exposes attempts,
configuration, usage, repository inspection digests and inspected HEAD.

Extraction adds **no model context** to the main agent. The updated Codex/Claude
prompt hooks and Pi/OMP extension request a relevant packet, normally at most
8 items and 6,400 UTF-8 bytes, within a 1.5-second host deadline. The packet is
whole-entry JSONL with a historical/untrusted-context notice. The actual token
count depends on language/tokenizer; the bound is bytes, not a promised token
count. A slow or unavailable query yields no injected memory and host work
continues. Context injection caps semantic retrieval at the smaller of the
configured query timeout and 750 ms, reserving time within the host deadline for
FTS fallback and CLI/IPC. Ordinary search retains its configured semantic timeout.
The host uses the first 1,000 prompt characters. FTS builds an OR query from at
most 128 deterministically ordered unique terms instead of rejecting longer
prompts; semantic embedding still receives the complete bounded query. The
4,096-byte query input limit remains in force.
Agentix captures session events and uses the installed host integration
for injection; it does not duplicate the packet inside Engine prompts.

The receiving host deduplicates memory ID/revision per session, both online and
offline. The service caches prepared packets for same-turn retry but does not
record their generation as delivery. A same-turn receipt is checked before
retrieval using a read-only existence probe; a miss does not acquire a SQLite
writer lock. Hits and final receipt creation are still checked inside the
transaction to resolve races. Empty receipts are hits; a hit reports `context_cache`. Referenced
memories are batch-revalidated for Project, revision, status and expiry before
rendering within the current byte budget. Unchanged receipts refresh their
timestamp at most hourly. Background maintenance removes receipts older than
30 days every minute, at most 1,000 rows per receipt table per pass.
A context request that times out before the hook accepts its text cannot
suppress memory in the next turn; old service-side delivery markers are ignored. Direct `memory context` CLI reads can return the same entries on later
turns because they do not confirm host injection. Host receipts persist only
metadata, not memory content. The hook accepts the text before asynchronously
publishing its receipt, without another deadline race between those two actions.
Receipt write failures or a host crash can cause a safe duplicate on a later turn.
Host delivery and receipt persistence are not an atomic transaction across a
process crash. Current user instructions and current repository evidence always
take precedence; memory is not authorization.

## Inspect, correct and forget

All commands accept global `--project PROJECT_ID` and `--json`. Without an explicit
Project, reads resolve the registered canonical cwd without registration or
migration. Reads can use the existing memory database in read-only mode when the
service is stopped; writes require the service.

```sh
taskix memory list --limit 20
taskix memory show mem_ID
taskix memory show mem_ID --revision 1
taskix memory source receipt_ID --message MESSAGE_ID --offset 0
taskix memory context 'deployment constraint' --session SESSION --turn TURN --budget 6400
taskix memory work 42
taskix memory receipt receipt_ID --wait-seconds 10
taskix memory retry 42
taskix memory reindex --limit 50
```

Follow returned cursors with `--after`; receipt waits are bounded to 300 seconds.
`source` returns metadata unless a message is selected, then a bounded UTF-8 page.
Historical intake is explicit and resumable:

```sh
taskix memory backfill job_ID --offset 0 --limit 20
```

Follow `next_offset` until `complete`. This handles legacy bound Job messages;
enabling memory does not scan all existing Jobs automatically. Source changes
cancel unfinished old extraction/consolidation work. Completed memories remain
versioned and are reconciled by subsequent evidence, not erased with a turn.

Create/update takes a JSON memory document, for example:

```json
{
  "title": "Offline deployments",
  "conclusion": "Production deployments must work without public internet.",
  "rationale": "Customer network policy; a hosted-only design was rejected.",
  "scope": "production",
  "conditions": ["Customer-managed installations"],
  "valid_until": null,
  "tags": ["deployment"],
  "kind": "user_decision",
  "evidence": []
}
```

```sh
taskix memory create --file decision.json
taskix memory update mem_ID --revision 1 --file decision.json
taskix memory forget mem_ID --revision 2 --reason 'User withdrew this decision'
taskix memory set-status mem_ID archived --revision 2 --reason 'Documented in repository'
```

Direct human input may omit evidence. Agent-attributed writes require valid,
literal same-Project evidence. The host passes its executor identity; do not
impersonate a human to bypass evidence checks. Consolidation retains every
candidate quotation, either verbatim or inside a longer literal quotation from
the same receipt and message. Longer quotations still require source validation;
changed attribution, omitted text and fabricated extensions are rejected.
Updates are revision guarded.
User decisions and assertions require at least one user quotation when evidence
is supplied. Assistant quotations can supply separately attributed proposal
context, but cannot establish a user decision alone. The extraction model must
still verify that the user actually selected or confirmed the quoted proposal.
Conflicts remain marked and searchable; superseded, archived, forgotten and
expired items are excluded from default recall. `list --all` and historical
`show` preserve inspection. Forget is a logical lifecycle operation with evidence
suppression, not physical erasure of source conversations or old versions.
Suppression includes evidence from every historical version of the memory, so
merges and edits cannot make older evidence eligible for replay/backfill again.
This applies when forgetting an existing memory, including versions saved by
older builds; it does not retroactively rebuild suppression records for memories
already forgotten by an older build.

Repository review periodically checks agent-authored active memories using the
same bounded worker pool. It schedules at most ten per Project per minute after
HEAD changes or the configured review interval (default one day, also covering
unversioned/dirty repositories). It requires a literal repository citation before
archiving documented content and preserves human edits. Set
`memory.agent.repository_review_interval_seconds = 0` to disable this additional
model workload. Human memories are never automatically archived by this loop.
Consolidation cannot archive existing memories; automatic archival is exclusive
to this citation-validated review path. Legacy archive proposals are rejected
without changing the target memory.

`taskix memory ask 'Why was the hosted approach rejected?'` runs a separately
limited, read-only Agent query with validated memory/source citations. Ordinary
search and context requests do not run an Agent loop.

## Obsidian and recovery

The service projects read-only memory notes into
`<documents.directory>/Projects/<project key>/Memory/<memory ID>.md`.
SQLite stores the complete memory content, properties, evidence and revisions.
Frontmatter contains identity and lifecycle metadata, such as `id`, `project_id`,
`revision`, `status` and timestamps. The complete `content` is rendered in the
Markdown body with subsections for each field; structured values use indented
YAML blocks. The `reason` is a separate body section. Neither `content` nor
`reason` is stored in frontmatter. Full synchronization repairs older note
layouts without changing database content or memory revisions.
Do not edit any part of these notes in Obsidian. File edits, including body text,
properties and managed metadata, are never imported into SQLite. Synchronization
restores the current database representation without changing the memory revision.
Use `taskix memory update`, `set-status` or `forget` for supported changes.

```sh
taskix memory sync --limit 20
taskix memory projection-status
```

These are logically read-only Markdown files, not an operating-system access
control boundary: a local editor can still change their bytes. Each note displays
a read-only notice. With the matching Taskix Sync plugin, saving an edited note
waits for 750 ms of quiet before checking and rolling the file back to SQLite
content. A read-only error is shown at most once per file per 30 seconds; rollback
still operates while repeated notices are suppressed. Opening an
edited note also checks it. Normal generated updates do not trigger errors;
lookup or write failures are reported without discarding the file. The plugin
uses the read-only `taskix memory document <vault-relative-path>` command, which
works without the memory service and validates the Project path. Taskix Sync passes
`TASKIX_MEMORY_ENABLED=true` only to the document lookup child process, allowing
existing notes to be protected without exporting the switch to Obsidian. Other
plugin commands retain the inherited environment. The service
prioritizes changed database revisions on each projection poll (default 5 s).
A full repair scan runs at startup and repeats after
`[memory.projection].reconcile_interval_seconds` (default `300`, range `1..86400`)
from the end of its last pass. Full scans advance in bounded pages on projection
polls, so larger vaults take additional time. They repair externally changed or
missing files even without the plugin. `taskix memory sync` still performs full
repair for its requested page; removing a note does not forget memory. If the vault itself is
removed, recreate a valid vault and configure its path before synchronizing.

Projection runs independently of extraction/search and retries missing-vault or
filesystem failures. Unchanged publication receipts are not rewritten; file I/O
runs in blocking tasks rather than on async runtime threads. `Recovery/` retains displaced file versions, including
unsupported local edits, and is not automatically pruned. Concurrent file changes
are preserved and retried rather than silently overwritten during publication.
Publication receipts survive restart, including a file installed before its
SQLite acknowledgement. Recovery-only edits and unrelated vault content are not
part of the authoritative memory database.

Use the existing [standalone backup script](taskix-backup.md) for ordered dual
snapshots, checksum/source-coverage verification and restore into a new directory.
Keep the entire memory database, including human revisions and forgotten-memory
markers. Restore does not change configuration, stop services or overwrite live
databases. SQLite is authoritative; a vault copy alone is not a memory backup.

## Verification and limits

See [memory acceptance](taskix-memory-acceptance.md) for reproducible commands,
coverage and measured scale results. Tests use real SQLite, local IPC, actual CLI
processes, host hooks and filesystem projections. Model/embedding HTTP responses
are mocked; this verifies contracts and recovery, not live provider availability,
semantic extraction quality or a production latency guarantee. Repository search
is bounded and reports incomplete coverage. Secret filename filtering and model
instructions reduce accidental capture but are not a content-classification
security boundary; only enable providers approved for the source material.

For a real Job-data comparison against frozen Codex reference labels, see the
[memory screening calibration](taskix-memory-calibration.md). Its empirical
recommendation is independent of routing confidence. The revised screening
context uses a `0.75` default; explicit service environment settings still override it.
The positive-gate experiment was rolled back because it missed reference memories.

The memory command definitions and shell completions are shared across platforms.
Unix uses a private Unix domain socket. Like Agentix's control socket, Windows
uses loopback-only TCP. The service binds `127.0.0.1` with an automatically
assigned port and publishes `tcp://127.0.0.1:<port>` plus a new instance UUID on
the second line of `<memory database>.tcp`. Clients reject non-loopback addresses
and verify the server's 16-byte instance greeting before sending any business
request. A stale file pointing at a reused port therefore fails closed. The
handshake is covered by the normal request timeout. A stable
`<memory database>.lock` file stays locked for the service lifetime, rejecting
another service for the same database. Shutdown removes the endpoint; a restart
replaces stale endpoint data left by a crash. Do not remove the lock file while
services are running. Each database receives its own port.

The instance greeting prevents accidental endpoint reuse; it is not user authentication.
TCP follows Agentix's local-machine trust boundary: it has no per-user peer
credentials or authentication, so other local users can access a known port.
Both transports share bounded JSON framing, concurrency limits and request
timeouts. Stalled clients are cancelled without blocking service shutdown.

On Windows, run `taskix memory serve` in a console or configure Task Scheduler to
launch it under the same user as the CLI/plugins. This command does not register a
Windows Service Control Manager service. Use the same absolute `--config` path
for the service and clients. The standalone Python/rclone backup script still
requires Unix; Windows IPC support does not change that script's platform scope.


Projection sync flushes note contents on all platforms. Unix also syncs the
parent directory; Windows does not apply Unix directory-fsync semantics.
SQLite remains authoritative for repairing interrupted or missing projections.

## Derived-data retention and performance checks

Background maintenance runs every minute. It scans at most 1,000 rows per derived
table per pass using persisted keyset cursors, so large backlogs drain over
multiple passes and resume after restart. It removes vectors and failure records
that no longer match a searchable memory revision and the current embedding
generation. Current retry state is retained.

Cancelled work without a work audit or repository-review reference is eligible
for deletion 30 days after maintenance first observes it. Upgrades start this
observation period instead of immediately deleting historical cancellations.
Pruned work IDs no longer appear in work diagnostics or queue counts. Original
sources, source heads, memory versions, suppressions, decisions, work audits,
and reviewed work are retained. Done/failed work is not automatically pruned.
SQLite reuses freed pages; this does not promise immediate file-size reduction
and does not run automatic VACUUM.

See [performance measurements](taskix-memory-performance.md) for repeatable
local benchmarks and their scope. Exact vector retrieval remains enabled;
the measured 10,000-record, 384-dimensional Project does not justify adding an
approximate index. Higher dimensions and larger datasets require fresh profiling.

Startup validates every retained source against the task database, using one
bounded primary-key query per page of at most 100 receipts. Full content and
instance checks remain mandatory; total startup work still grows with retained
history size. This reduces query round trips without claiming a measured speedup
or changing exact vector retrieval into approximate nearest-neighbor search.
