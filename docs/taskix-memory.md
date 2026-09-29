# Taskix project memory

Taskix remembers reusable project decisions, reasons and rejected alternatives,
constraints, external facts and verified lessons that cannot be found in the
repository. It keeps evidence and revisions, distinguishes user decisions from
assertions, observations and inferences, and does not treat brainstorming as a
confirmed decision. Repository facts, progress reports and internal tool chatter
are not useful memory candidates.

Memory maintenance runs in **`taskix memory serve`**, a separate process shipped
in the same executable and release as the task board. It uses the same Taskix
configuration file. The service is available on macOS/Linux; the existing task
board remains available on Windows. This delivery does not install or launch a
background service automatically.

## Enable and operate

Initialize Taskix and register the project normally. Add these sections to the
existing `~/.config/taskix/config.toml` (or the file selected by `TASKIX_CONFIG`
or `--config`):

```toml
[memory]
enabled = true

[memory.providers.openai]
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[memory.agent]
provider = "openai"
api = "responses"
model = "gpt-6-astra"
```

Provide the named environment variable to the **service process**, then run:

```sh
taskix memory doctor
taskix memory serve
# From another terminal in a registered project:
taskix memory status
taskix memory search 'offline deployment'
```

Use a process manager such as launchd or a systemd user unit to run the same
foreground command if desired. Give it an absolute executable/config path and
the required environment; shell login state and the main agent's subscription
are not inherited authentication. SIGTERM/Ctrl-C stops service background loops;
unfinished work resumes after its durable lease expires. One service can manage
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
loops finish with their original model/settings. Database paths, enabled state
and IPC connection/size limits require a restart; an incompatible reload keeps
the running configuration intact. `memory doctor` checks configuration,
credential availability, database identity and service availability without a
model request. Credential availability in doctor refers to the CLI environment;
`service.provider_errors` describes the running process.

## Models and embeddings

Extraction, consolidation and repository review share `[memory.agent]`; each
work item and retry starts a fresh Agent context. Clients and connection pools
are reused, conversations are not. Models are called through explicit APIs,
with structured tool responses and application-side evidence validation.
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

Query vectors have a 64-entry, one-minute, project/generation-scoped cache;
results themselves are not cached, so forgetting and revisions take effect on
subsequent reads. Provider failures/timeouts fall back to FTS. Exact vector
scanning uses 256-row pages and bounded top-k storage. Its CPU/I/O cost still
grows with the number and dimensions of vectors in the selected Project.

## Source delivery, concurrency and cost

Visible turns are captured transactionally in the task database, together with
an immutable outbox receipt. Explicit Project/Job ownership and canonical cwd
resolve scope; unknown ownership is not guessed from the latest Job. Worktrees
share their canonical Project. Pure discussions can be sources without a Job.
Late replies and corrections create new source revisions. Deleted/expired
original discussion records do not remove already retained evidence. Project
archival stops new background maintenance; history remains explicitly readable.

The service commits receipt, evidence and queue entries before acknowledging the
outbox. Acknowledgement means **received**, not extracted. No model work runs
inside the main agent or task transaction. Intake failures are retried separately
from the forward replay scan. Durable retries, leases and source revisions fence
stale workers. Startup checks stored sources against the task history and replays
newer retained receipts, including acknowledged inputs after an older memory
restore. Source snapshots are deliberately retained; there is no automatic prune.

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
three attempts. Lowering the context limit below a work item's needs causes a
reported failure rather than truncating evidence. `work` exposes attempts,
configuration, usage, repository inspection digests and inspected HEAD.

Extraction adds **no model context** to the main agent. The updated Codex/Claude
prompt hooks and Pi/OMP extension request a relevant packet, normally at most
8 items and 6,400 UTF-8 bytes, within a 1.5-second host deadline. The packet is
whole-entry JSONL with a historical/untrusted-context notice. The actual token
count depends on language/tokenizer; the bound is bytes, not a promised token
count. A slow or unavailable query yields no injected memory and host work
continues. The service's optional embedding timeout does not extend that host
deadline. Agentix captures session events and uses the installed host integration
for injection; it does not duplicate the packet inside Engine prompts.

Memory ID/revision is deduplicated per session. Same-turn retries can replay their
packet; later turns do not repeatedly inject unchanged entries. Offline hooks
persist only receipt metadata, not memory content. Current user instructions and
current repository evidence always take precedence; memory is not authorization.

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
impersonate a human to bypass evidence checks. Updates are revision guarded.
Conflicts remain marked and searchable; superseded, archived, forgotten and
expired items are excluded from default recall. `list --all` and historical
`show` preserve inspection. Forget is a logical lifecycle operation with evidence
suppression, not physical erasure of source conversations or old versions.
Suppression prevents replay/backfill from recreating the same evidence.

Repository review periodically checks agent-authored active memories using the
same bounded worker pool. It schedules at most ten per Project per minute after
HEAD changes or the configured review interval (default one day, also covering
unversioned/dirty repositories). It requires a literal repository citation before
archiving documented content and preserves human edits. Set
`memory.agent.repository_review_interval_seconds = 0` to disable this additional
model workload. Human memories are never automatically archived by this loop.

`taskix memory ask 'Why was the hosted approach rejected?'` runs a separately
limited, read-only Agent query with validated memory/source citations. Ordinary
search and context requests do not run an Agent loop.

## Obsidian and recovery

The service projects read-only memory notes into
`<documents.directory>/Projects/<project key>/Memory/<memory ID>.md`.
SQLite stores the complete memory content, properties, evidence and revisions.
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
works without the memory service and validates the Project path. The service repairs changed or missing files on its next
projection pass; removing a note does not forget memory. If the vault itself is
removed, recreate a valid vault and configure its path before synchronizing.

Projection runs independently of extraction/search and retries missing-vault or
filesystem failures. `Recovery/` retains displaced file versions, including
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
