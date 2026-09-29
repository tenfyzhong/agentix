# Project memory architecture and contract

This document records the implementation contract. See [operations and configuration](taskix-memory.md)
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

## Architecture

One repository and release ship the existing `taskix` executable. An independent
resident process, `taskix memory serve`, owns maintenance and semantic writes.
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

## Memory, search and lifecycle

Each atomic memory carries conclusion, rationale, scope, conditions, type, tags,
evidence, revision and timestamps. Preserve versions and relationships for
conflicts and supersession. Default retrieval excludes forgotten, superseded and
archived content. Explicit historical inspection remains possible. Forgetting
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
