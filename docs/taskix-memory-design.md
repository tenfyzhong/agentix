# Project memory architecture and contract

This document records the implementation contract. See [operations and configuration](https://github.com/tenfyzhong/agentix/wiki/Taskix-Memory)
and [acceptance evidence](taskix-memory-acceptance.md) for commands, limits and verification.

## Product boundary

Remember reusable decisions, their reasons and rejected alternatives, project
constraints, external facts, and verified lessons that the repository cannot
answer. Do not reproduce code structure, checked-in documentation, transient
progress, logs, or speculative brainstorming as established facts. Distinguish
user decisions, user assertions, observations, and inferences. Capture confirmed
conclusions from discussions even when no Job exists.

Maintenance is automatic; no item-by-item approval is required. Current user
instructions and current repository evidence take precedence. Memory is historical
context, never authorization to execute an old instruction.

## Memory identifiers and migration

Memory IDs use `mem_YYMMDDHHmmSS_<UUIDv7>` with the computer's local time at
creation and a 32-character unique suffix. A missing local time-zone resolution
fails the write rather than silently using UTC. Sort the IDs lexically for local
creation order; the UUID suffix distinguishes records created in the same second.
IDs remain fixed when the computer's time zone changes. Daylight-saving clock
rollback can repeat local timestamps; the suffix still preserves uniqueness.

Memory schema 2 migrates legacy `mem_<UUIDv7>` IDs once, deriving the local
prefix from each record's original `created_at`. The immediate transaction
preserves rowids, content revisions, evidence, supersession links, FTS, vectors,
suppressions, review references and context receipts. It remaps structured work
references and fences running consolidation leases for retry. Source snapshots,
evidence quotations and model audit text remain verbatim. A collision or any
migration error rolls back the whole transaction and schema version.

Upgrade the service and CLI together, stop the old service before migration,
and retain a SQLite online backup. Old writable binaries reject schema 2.
Upgrade `scripts/taskix-backup.py` as well: backup and restore support schemas
1, 2, 3 and 4, retain the archived schema version, and reject unknown versions.
Offline reads support schemas 1, 2, 3 and 4 without migrating. Reset pagination cursors
after migration and use the new IDs for CLI mutations. On the next projection
sync, renamed notes publish under the new ID and preserve old file bytes in
`Memory/Recovery/`. An existing destination with different contents is a
conflict: both files remain available and synchronization can retry after the
conflict is resolved. Upgrade the embedded Obsidian plugin as well so new
filenames keep their read-only protection.

## Architecture

One repository and release ship the existing `taskix` executable. An independent
resident process, `taskix serve`, owns maintenance and semantic writes.
`agentix-task` retains task state and conversation capture. `agentix-memory` owns
memory domain rules, storage, retrieval, providers, workers, projection and API.
Neither library depends on the other. A source adapter in `taskix` connects them.

Keep `tasks.sqlite3` and `memory.sqlite3` separate. Both contain authoritative
data: human memory edits cannot be reconstructed from task history. Only search
indexes and vectors are disposable. Task operations never require the daemon,
model credentials, or a reachable provider. Memory queries and extraction must
remain usable when the Obsidian vault is unavailable.

Use the existing configuration file, with memory-specific sections validated
only by the capabilities that use them. Separate provider connections, Agent
model, embedding model, service limits and retrieval budgets. Models are explicit;
do not silently downgrade. A task snapshots its configuration; reload affects
subsequent tasks. API keys are environment references and belong to the service
environment, independently of the main Agent's subscription or login.

## Source delivery and recovery

Save a changed turn and an immutable source snapshot in the same task database
transaction. Identify the source database instance, canonical Project, session,
turn and content revision. Resolve ownership explicitly; never select the most
recent Job. Worktrees share canonical Taskix Project identity.

The memory database transaction saves the receipt, evidence snapshot and work
item before acknowledging the task outbox. Retries deduplicate by source identity.
Acknowledgement is per receipt, not a maximum completed sequence: parallel workers
can finish out of order. Pending snapshots survive discussion retention, and
ingested evidence remains available after the original discussion expires.

Ordered replay has a separate durable checkpoint; the maximum stored sequence
does not prove that earlier inputs arrived. Advance only after a successful
ingest, stop on failure, and resume from that checkpoint after restart. Legacy
databases without a checkpoint replay idempotently from zero to fill old gaps.

Late replies and corrections create new versions. Source attachment, deletion,
archival and Project path changes need explicit behavior. Historical extraction
is manually triggered, bounded, resumable and lower priority; enabling memory
does not automatically scan historical Jobs. Backfill must include legacy bound
conversations and expose scope and progress.

Crash tests cover before receive, after receive before acknowledgement, worker
failure, expired leases, restart, source rollback and paired database restore.
Source retention/pruning must not make an older memory restore silently lose
acknowledged inputs. Backups extend the existing standalone script and retain
single-database compatibility. Two files in one tar archive alone do not prove a
consistent recovery point; the implementation must enforce and test a recovery
protocol with database identity and coverage checks.

## Workers and model tools

Use a bounded persistent queue with fair Project scheduling, retries, deadlines
and generation-fenced leases. Parallel extraction produces candidates. One
consolidation loop per Project resolves candidates against existing memories;
different Projects consolidate concurrently. Model requests never hold database
transactions or connection leases. CLI edits advance revisions and fence stale
model results; Obsidian is a read-only projection.

Extraction waits for a configurable short settling window after claim; rapid
source revisions cancel superseded derived work while all original receipts
remain durable. Poll lease validity during execution to drop obsolete model/tool
futures promptly. Do not deduplicate solely by message text: added context can
change the interpretation of an unchanged selection.

Every task and retry creates a fresh Agent context. Workers may reuse clients and
connections, but never another task's conversation. Bound source input, tool
results, tool calls, output and duration. Tools read scoped evidence, memory and
repository files; they do not expose arbitrary SQL, shell execution, repository
writes or task lifecycle mutations. Record the repository revision inspected.
Do not capture internal memory Agent conversations into the task outbox.

An extraction loop can discover preceding sources with `source_neighbors`,
anchored at its current receipt. Pages contain at most eight current turn
revisions in the same Project, source instance and session. Turns retain their
first receipt sequence ordering across later edits or Job attachments. Persist
that order and current source summary in a scoped index, transactionally backfill
older databases once, and read message bodies only on demand.
`source_read` with a null message ID lists up to 32 message identifiers per page;
with a message ID it reads bounded text. This also recovers adjacent context for
legacy backfill receipts containing one message each. Context-dependent choices
must inspect the proposal rather than guess what a short approval refers to.
Jev's unresolved-reference fallback remains conservative; Jev does not gain a
separate conversation or repository access path.

Support explicit Responses and Chat Completions API adapters where applicable.
Validate structured proposals and their source references in application code.
User-decision evidence requires a user quote, while allowing assistant quotes as
supporting context; assistant-only decisions remain invalid.
Do not let the model assign stronger evidence than the source supports. Normal
queries do not run an Agent; optional deep queries run a separately limited,
read-only loop and return traceable sources.

## Consistency and incremental compaction

Extraction emits every supported independently replaceable fact as its own memory.
A structured `fact` contains a canonical `entity`, one `attribute`, a bounded
array of `{name, value}` qualifiers, and a separate `value`. Project, normalized
entity/attribute and sorted qualifiers identify the fact; value, title and
record ID do not. Conditions that distinguish applicability belong in qualifiers.
Deployment path, autostart, domain, client SNI and independently configurable
protocol/port settings remain separate. Repository-discoverable information and
unconfirmed brainstorming remain excluded.

Before a background write, consolidation receives exact indexed matches for that
identity plus bounded related lexical snapshots. The model reuses canonical
identity names when source wording changes. This is not exhaustive semantic
matching across arbitrary synonyms. A new fact becomes active; the same identity
and value merges evidence; a confirmed later replacement supersedes the old
record and creates a new active record. Original source dates and explicit
replacement statements establish precedence; newer ingestion alone does not.
Unresolved incompatible claims become conflicted and leave ordinary retrieval.
The same invariant applies to direct create, update, supersede and status writes:
no active fact may coexist with a conflicted claim of the same identity. Checks
run at the transaction boundary so multi-record reconciliation can retire all
claims atomically. Rejected writes roll back content, versions and indexes.
Manual resolution must retire the other claims before activating the selected
record; changing only one conflicted record cannot publish a confirmed value.
Conflict diagnostics remain available through the read-only `memory_conflicts`
tool and explicit ID/history inspection.

A partial legacy replacement can retain unrelated facts; compact migrates legacy
mixed records into atomic parts. Literal prior evidence must remain available.
Fact identities and values are immutable within a record: changing either
requires a new record. A partial unique index permits at most one active record
per Project/fact identity. All mutations, version writes, dirty observations and
work completion share one fenced transaction. Stale revisions, older replacement
evidence, unrelated fact merges and evidence loss roll back the whole proposal.
Human-authored records are protected from automatic replacement. Successful
write-time reconciliation marks its atomic outputs observed, avoiding redundant
automatic compact work. A deep answer citing conflicted records must report
insufficient evidence.

Automatic forgetting is limited to a current, literal user message naming the
exact memory ID or title: `Forget memory <ID or TITLE>.` or `忘记记忆 <ID or TITLE>`.
The full message, source date, role and candidate evidence must match. Historical,
negated and quoted requests do not authorize forgetting. Suppression covers every
stored version to prevent replay. Manual forgetting retains its revision guard.
Compaction never authorizes forgetting.

Semantic compaction is asynchronous consolidation, independent of derived-data
pruning and repository review. A dedicated daemon loop wakes on memory writes,
compact work completion, startup recovery and configuration reload. It drains at
most ten indexed dirty records per unarchived Project per pass. The durable
per-revision dirty flags also track one-time historical processing and survive
restart; there is no rotating scan or periodic reassessment of unchanged records.
Only searchable, nonexpired Agent records qualify for model work. Human and
retired revisions do not enter the dirty candidate index. Records expiring while
waiting are drained in pages of ten and marked observed without model calls;
this prevents expired candidates from being rescanned on every later write.
The candidate index excludes suspended failures. The default dirty debounce is
30 seconds. The loop arms a timer only for an actual outstanding dirty deadline;
with no eligible dirty work it waits for notifications without compact database
scans or model calls. Setting `compaction_enabled = false` disables automatic
scheduling. Re-enabling it through reload resumes durable dirty work. Scheduling
records a durable work ID and avoids duplicate pending/running work across restart
or manual requests. A changed revision waiting behind an old seed is woken when
that work finishes or exhausts its lease.

Compaction work has priority 3 within its Project, below live extraction/consolidation and backfill,
and uses the existing per-Project consolidation lane and provider/Agent budgets.
Legacy seeds without a structured fact are split into up to sixteen independent
parts. Legacy splitting receives complete bounded memory/source snapshots and exposes
only its submission tool; repository review remains in its independent lane.
It uses at most four model steps (or a lower configured limit) for the initial
proposal and validation corrections, without repeating evidence reads. Split proposals reference preloaded quotations
by memory ID and zero-based quote index; the worker restores exact receipt/message
IDs and original text before validation. The submission schema permits only
actually supplied memory IDs and quote indices; invalid indices report their
available range. Unknown records/indices are correctable
proposal errors. Missing evidence returns up to sixteen exact memory/quote references
and the total missing count for correction. The submission schema requires a non-null
fact in each part. The seed is supplied once rather than duplicated as a candidate. Its current
conclusion is supplied as `migration_scope`; example attributes and a formerly
broader title do not require unrelated facts or reconstruction of retired facts.
Keeping a complete quotation is separate from extracting every fact it mentions.
Related assessments are restricted to supplied IDs and snapshot revisions; they can
keep records or retire whole legacy agent records, while atomic and human records
can only be kept through this assessment path. Duplicate creates against supplied
active facts receive corrective feedback before the fenced transaction; atomic
updates still use part-level reconciliation. Protocol, container port and published
port are separate attributes.
Expanded proposals retain the original submission byte limit,
and all original evidence-retention fences still apply. Request/task
timeouts, tool limits and retries remain unchanged. Each part creates a new fact or reconciles an existing atomic version;
merge requires identical identity and value. The seed and overlapping legacy
records become superseded only when their evidence is preserved among the parts.
Every part records `derived_from` IDs; an indexed one-to-many lineage table also
retains each original revision. Superseded originals and all versions remain
inspectable. A legacy record's `superseded_by` points to the first part, while the
parts and lineage preserve the complete split. Atomic seeds use normal same-fact
reconciliation. Supplied evidence dates and author roles avoid redundant source reads. Invalid
literal quotations and assistant-only user decisions receive corrective feedback
before the transaction; the store repeats evidence validation when committing. Different
attributes may legitimately share a hostname; identity/value matching determines
duplicates rather than counting text occurrences. Human protection, revision
guards and the sixteen-quotation limit per part still apply.
Commit rechecks the seed expiry as well as its revision, actor and status, so a
seed expiring during model execution cannot be revived by the proposal.
A stale seed is completed without a model call; revisions produced by a successful compact
are marked observed to prevent recursive scheduling. Failures retain normal retry
limits; permanent provider 4xx rejections, except 408 and 429, fail immediately.
They remain inspectable; exhausted work requires a retry, a changed memory revision,
or explicit manual compaction rather than being re-created automatically.

The manual API scans an ID page of 1–100 records (default ten) and returns scanned
and scheduled counts, work IDs and `next_after`. It ignores the observed-revision and debounce checks,
while retaining all eligibility and duplicate-work guards. This command enqueues
work; it does not wait for model completion or imply exhaustive consolidation.
Normal search, context and deep-query reads never trigger compaction. Source
snapshots and version history remain durable; this is not physical storage
reclamation or a retention policy.

Provider requests require tool calls when tools are supplied. The final Agent
step exposes only the submission tool, retaining the original step/time budgets.
Non-success HTTP diagnostics retain only bounded recognized status/schema fields;
arbitrary provider messages are omitted. The known location rejection explains
that the upstream proxy route needs attention without exposing credentials or
request contents.

## Memory, search and lifecycle

Each atomic memory carries its structured fact, conclusion, rationale, scope, conditions, type, tags,
evidence, revision and timestamps. Preserve versions and relationships for
conflicts and supersession. Default lexical/vector search, context and lists include only active,
unexpired content. Conflicted, forgotten, superseded and archived records are
excluded. Explicit historical inspection remains possible. Forgetting
removes search visibility and atomically creates suppression records from every
historical version of that memory. Merges and edits can replace current evidence,
so suppressing only the latest version would let replay/backfill resurrect the
original decision. Read existing version history when forgetting, including
versions written by older builds; no new extraction is required. This operation
does not retroactively rebuild suppressions for records already forgotten by an
older build. Retire material that becomes documented in the repository from
regular injection.

Embedding batches use a bounded, rotating cross-Project scheduler, with one
active batch per Project and independent cooldowns. Slow Projects occupy only
their own slots. Provider concurrency limits still apply; hot reload cancels old
batches before admitting the replacement runtime.

Embedding maintenance isolates input-rejected batches and persists per-record,
revision/generation-fenced retry state. Three failed attempts suspend automatic
retries until an explicit reindex, memory edit or model-generation change.
Other failures back off without multiplying provider requests. When dimensions
are discovered from the first response, a later response incompatible with the
current generation's resolved dimension is a failure, not a successful empty
write. Persist its retry state and expose the error through status. Individual
failures during a split batch do not abort the remaining records. A delayed
response for a superseded generation is discarded without changing the new
profile, vectors or retry state. FTS remains available, and excessive lexical
query terms are bounded rather than disabling an otherwise valid context request.

FTS5 is usable without embedding. Index and query share versioned Chinese word
segmentation and English identifier handling; use weighted BM25. SQL scopes
Project and lifecycle before retrieval. Vector recall is independent of FTS;
combine ranks with RRF. Initially use SQLite vector storage and scoped exact
cosine search in Rust, measured against realistic Project sizes. Maintain exact
top-k with a bounded worst-first heap and one final deterministic score/ID sort.

Embedding has independent provider configuration, including OpenAI-compatible
embeddings and Ollama. Generate asynchronously. Fence writes by memory ID,
content revision and embedding profile generation. Model, dimension or input
template changes create a new generation; never mix incompatible vectors. Check
finite values and consistent dimensions. Partial indexing and provider failures
leave FTS usable with explicit degradation and index progress.

Use bounded pages, scoped indexes and bounded caches. Online queries have
independent resource limits from maintenance and historical backfill. Return
pending work and coverage; optional receipt-based waits have a deadline.
Coalesce at most 64 in-flight query embeddings per runtime by Project, generation
and query. Keep caller deadlines independent and bound shared work by the query
timeout; cache only successful vectors. Dropping an old runtime cache cancels its pending work.
Cache the Project list for five seconds; idle indexing backs off to 30 seconds.
Post-commit notifications wake affected Projects, with periodic recovery for
other processes and notification loss. Extraction workers similarly back off to
five seconds; a queue watch generation prevents lost wakeups from stale empty
claim results. Provider-wide background admission leaves one online embedding
query slot when total concurrency is at least two; the total limit remains fixed.

## User and host interfaces

The CLI exposes service operation, search/show/list/context/source inspection,
status/doctor, manual updates/forgetting, reindexing and historical backfill.
Semantic writes use the service. Ordinary reads can fall back to shared read-only
FTS when the service is unavailable. Protect local IPC with per-user isolation,
single-instance ownership and bounded versioned requests.

Codex, Claude, Pi, OMP and Agentix capture visible conversation through existing
paths and consume a small relevant context plus on-demand search. Enforce byte
or conservative token budgets and deduplicate memory ID plus revision within a
session at the receiving host. Packet generation is not delivery: service-side
same-turn caching must not suppress a later turn after a timeout or disconnect.
Check same-turn receipts with a read-only probe before retrieval, including empty
packets. A miss avoids a writer lock; the final transaction still checks for a
concurrent winning receipt. Batch-check
referenced revisions and lifecycle under the receipt transaction, preserving
rank order and the current byte budget. Avoid unchanged receipt writes except
for hourly timestamp refresh. Perform bounded 30-day receipt cleanup separately
every minute (up to 1,000 rows per table), including while queries are idle.
Reserve time for lexical fallback within the host deadline.
Do not delay the main Agent for extraction. Queries and maintenance do
not change Job ownership or Jev routing semantics.

Obsidian memory notes are read-only projections of SQLite. Never import file
edits: restore database content on synchronization without creating a memory
revision. Supported mutations go through the CLI/API. Preserve replaced files
in Recovery and retain durable publication receipts for filesystem retries.
Missing files are regenerated and never implicitly forget memory. A read-only
notice explains that ordinary Markdown cannot prevent local filesystem writes.

## Acceptance

Apply TDD to each behavioral component. Integration tests use real SQLite,
migrations, CLI processes, local service transport and host entrypoints. Mock
only external model/network providers. Verify structured tool loops, context
isolation, source attribution, cross-Project isolation, retries, fairness,
revision fencing, cache invalidation, degraded retrieval, projection conflicts,
backup restoration and disabled-feature compatibility.

Measure scoped query plans, paging, resource bounds and online query latency
during maintenance. Record exactly what fixtures and mocks prove; mock success
does not establish live provider behavior or semantic extraction quality. Before
delivery, audit every requirement above against current code and passing tests,
publish configuration/operation/recovery documentation, and regenerate CLI
completions. Partial green tests do not establish full implementation.

## Jev preflight

Extraction reuses `TASKIX_JEV_ENABLED`, URL, API key and model from the host
integration. Its independent `TASKIX_MEMORY_JEV_MIN_CONFIDENCE` threshold defaults
to `0.75` and never inherits `TASKIX_JEV_MIN_CONFIDENCE`.
A disabled or invalid configuration uses
the model directly. Screening runs inside the service, never in the main agent;
only confident `skip` bypasses extraction. Uncertain, low-score, malformed and
service-failure results use the model. The
source stays durable and completion remains fenced. The existing v2 Jev metrics
file records `memory_triage`, including scores and fallback reasons, without
storing conversation content. No separate triage config section is introduced.

## Incremental projection maintenance

Publish pending database revisions on the regular projection poll. Skip receipt
updates when the published revision/hash and recovery state already match.
Run a separate lower-frequency, paginated full reconciliation to detect file
edits and deletion; dirty database tracking alone cannot detect these changes.
Keep explicit CLI synchronization as a full repair of its requested page. Perform
blocking file operations outside async runtime threads, while preserving the
publisher lock and prepared/publication receipts for crash recovery.


## Derived-data maintenance

Use persisted keyset cursors to bound candidate scans and deletions to 1,000 rows
per table per minute. Remove obsolete vector generations/revisions and stale
embedding failure records. Retain original sources and all decision/version
history. Cancelled work may be removed after a 30-day first-observed retention
period only if no work audit or repository review references it. Keep done,
failed and audited/reviewed work. This frees reusable SQLite pages without an
automatic VACUUM. Index review lookups by work ID.

Keep exact vector search for the currently measured scale; see
[performance evidence](taskix-memory-performance.md). An approximate index
requires evidence that exact scanning dominates the latency budget and explicit
recall/lifecycle validation.

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

### Atomic-fact schema upgrade

Memory database schema 3 adds the fact identity/uniqueness index and split lineage.
Upgrade the CLI and memory daemon together; older writers do not understand the
new identity invariant. Existing schema 1/2 databases migrate atomically, preserving
sources and versions. Previously settled legacy mixed records are marked dirty
once so they can be split, including records with an explicit null fact. Restart
under schema 3 does not reopen unchanged assessed records. Projection metadata is
reset once to render fact blocks and lineage. Migration itself does not call a
model; subsequent background or manual compact performs the semantic split.


### Cancelled source Jobs

Task schema 21 records entry into `CANCELLED` in a durable cancellation outbox,
inside the same SQLite transaction as the Job update. A database trigger covers
CLI cancellation, Inbox cancellation and withdrawal, including existing writers
that already hold a connection. The history survives reopening and deletion of
the Job and ordinary event retention. Migration captures currently cancelled
legacy Jobs once; cancellations followed by reopening before this upgrade cannot
be reconstructed from current Job state.

Memory schema 4 adds `invalidated`, indexed receipt-to-memory and Job-to-receipt
relationships, persistent turn ownership and cancellation tombstones. Current
facts, including merged evidence and compacted children, become invalid when any
of their supporting sources belongs to a cancelled Job. A split uses the child's
own evidence, so unrelated facts from a mixed legacy record are not invalidated
merely because they share a compaction parent. The new revision records
`source_job_cancelled`, the Job ID and cancellation time; original versions,
evidence and supersession links remain available for audit. FTS/vector indexes
and projection revisions update through the normal memory write transaction.

Pending and running extraction/consolidation work loses its lease generation.
Every searchable write rechecks evidence inside its transaction, including
manual writes and compaction merges. Replay stores revoked receipts for audit
without scheduling extraction. Turn ownership also fences receipts captured
before a discussion was attached, even if the cancellation arrives before those
receipts. Invalidated memories cannot be reactivated through lifecycle changes;
a new independently supported fact must use uncancelled evidence. Reopening a
Job does not restore its old facts, and cancelling a replacement does not revive
its superseded predecessor. Worker cancellation or failure alone never revokes
business facts.

The daemon consumes cancellation events in pages of 100 and persists progress
only after applying each event; retries are idempotent. Source pages and affected
memory batches are bounded to 100 records, with indexed provenance lookup rather
than a periodic full-memory scan. Applying one Job's invalidation is transactional;
its total work and writer-lock duration still grow with that Job's affected facts.
The schema migrations build provenance indexes once and enumerate existing data;
ordinary reopen does not repeat those backfills.

Online operations synchronize cancellation progress before use and recheck it
after potentially slow model/embedding calls. An unfinished event backlog fails
closed and resumes on the next call. Offline search, context and default list
validate candidate provenance against the task database read-only, including
cancellations after the daemon stopped and later turn attachment. Offline filtering
can return fewer than the requested limit; it does not rewrite memory history or
claim that the stored status has already changed. Deep answers also reject direct citations of cancelled sources, even without a
memory citation. Explicit historical reads remain available. A query started after a committed cancellation cannot return its facts;
this does not retract text already delivered to an Agent before cancellation.

Upgrade Agentix, Taskix and the daemon together. Older binaries reject task schema
21 and memory schema 4 on open. The backup tool accepts memory schema 4 and
preserves the databases without downgrading them.

### Auxiliary metadata initialization

Projection and compaction metadata are backfilled once per database, guarded by
`memory_auxiliary_version` in the migration transaction. Existing publication and
compaction progress are preserved. A failed backfill rolls back the marker and
all earlier changes so reopening can retry safely. Subsequent writable opens do
not enumerate memories for these backfills or recreate the failure trigger;
normal writes maintain the auxiliary rows. This does not remove the service's
separate retained-source validation at startup described above.

## Service lifecycle

Run `taskix serve` to host Taskix services (currently project memory). The former
`taskix memory serve` and `taskix memory reload` commands have been removed.
Enable memory with `TASKIX_MEMORY_ENABLED=true` or `1`, then use `taskix reload`
to reload the running service's startup configuration file for future work.
`--config` selects the client configuration used to locate that service. Reload
does not require a registered Project. Invalid configuration leaves the active
configuration unchanged; storage paths, IPC limits and logging changes require a restart.
On Unix, only `serve` loads the login shell environment; restart after changing
exported credentials, proxy settings, or memory enablement.

### Service logs

`taskix serve` writes tracing logs to stderr. Its optional `[logging]` and
`[logging.file]` sections follow Agentix's configuration: `level = "info"`,
`enabled = false`, `path = "~/.local/state/taskix/taskix.log"`,
`rotation = "daily"`, and `max_files = 7`. Set `logging.file.enabled = true`
to also write files; parent directories are created automatically. `RUST_LOG`
overrides the configured filter. Normal CLI commands do not initialize logging.

Rotation supports `never`, `minutely`, `hourly`, and `daily`. Rotated filenames
append a UTC timestamp to the configured filename; log entries use local RFC 3339
timestamps. File output has no ANSI colors and uses a non-blocking writer, flushed
on graceful exit or startup failure after logging initialization. `max_files`
limits retained rotated files, not bytes; `never` leaves one unbounded file.

Logs include service startup, successful reload, shutdown, fatal errors, and
changed background-operation errors. Repeated identical background errors are
suppressed until recovery or a changed error. Logging starts after Unix login-shell
re-execution, so environment-loading diagnostics still appear only on stderr.
Changing logging configuration requires restarting the service; `taskix reload`
rejects such changes without replacing the active configuration.
