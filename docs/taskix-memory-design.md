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
transactions or connection leases. Obsidian and CLI edits advance revisions and
fence stale model results.

Every task and retry creates a fresh Agent context. Workers may reuse clients and
connections, but never another task's conversation. Bound source input, tool
results, tool calls, output and duration. Tools read scoped evidence, memory and
repository files; they do not expose arbitrary SQL, shell execution, repository
writes or task lifecycle mutations. Record the repository revision inspected.
Do not capture internal memory Agent conversations into the task outbox.

Support explicit Responses and Chat Completions API adapters where applicable.
Validate structured proposals and their source references in application code.
Do not let the model assign stronger evidence than the source supports. Normal
queries do not run an Agent; optional deep queries run a separately limited,
read-only loop and return traceable sources.

## Memory, search and lifecycle

Each atomic memory carries conclusion, rationale, scope, conditions, type, tags,
evidence, revision and timestamps. Preserve versions and relationships for
conflicts and supersession. Default retrieval excludes forgotten, superseded and
archived content. Explicit historical inspection remains possible. Forgetting
removes search visibility and creates suppression records so replay/backfill
cannot resurrect the same evidence. Retire material that becomes documented in
the repository from regular injection.

FTS5 is usable without embedding. Index and query share versioned Chinese word
segmentation and English identifier handling; use weighted BM25. SQL scopes
Project and lifecycle before retrieval. Vector recall is independent of FTS;
combine ranks with RRF. Initially use SQLite vector storage and scoped exact
cosine search in Rust, measured against realistic Project sizes.

Embedding has independent provider configuration, including OpenAI-compatible
embeddings and Ollama. Generate asynchronously. Fence writes by memory ID,
content revision and embedding profile generation. Model, dimension or input
template changes create a new generation; never mix incompatible vectors. Check
finite values and consistent dimensions. Partial indexing and provider failures
leave FTS usable with explicit degradation and index progress.

Use bounded pages, scoped indexes and bounded caches. Online queries have
independent resource limits from maintenance and historical backfill. Return
pending work and coverage; optional receipt-based waits have a deadline.

## User and host interfaces

The CLI exposes service operation, search/show/list/context/source inspection,
status/doctor, manual updates/forgetting, reindexing and historical backfill.
Semantic writes use the service. Ordinary reads can fall back to shared read-only
FTS when the service is unavailable. Protect local IPC with per-user isolation,
single-instance ownership and bounded versioned requests.

Codex, Claude, Pi, OMP and Agentix capture visible conversation through existing
paths and consume a small relevant context plus on-demand search. Enforce byte
or conservative token budgets and deduplicate memory ID plus revision within a
session. Do not delay the main Agent for extraction. Queries and maintenance do
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
