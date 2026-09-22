# Taskix Manager

One plugin package connects Codex, Claude Code, Pi, and Oh My Pi (OMP) to the independent `taskix` task board. Lifecycle configuration is bundled; do not copy it into each project's settings.

Hosts stage visible user prompts and agent replies in SQLite session drafts before a Job exists. Codex/Claude stage the current transcript turn at the first tool call and refresh it on Stop, interruption, and session end; Pi/OMP stage the prompt before execution and replies at `agent_end`. Agentix also stages native user items and completed turns. Stable turn/message IDs prevent duplicate capture across retries and cooperating entrypoints. Tool calls/results, reasoning, and injected host context are excluded by the standalone plugin. Transcript reads remain bounded to the current turn, using backward 64 KiB chunks.

When discussion leads to implementation, the agent selects the related pending turns and attaches them atomically on Job creation or follow-up, or explicitly to an ACTIVE Job. The generated **Conversation** preserves the selected original inputs and replies in order, including superseded alternatives and Codex Plan-mode questions and answers. A new Job's **Prompt** comes from the earliest selected request; follow-up preserves its original Prompt. Unrelated turns remain pending. Discussion alone produces no Job note. Drafts survive restart and expire after 30 days of session inactivity; recorded Job history is retained. Existing Jobs are not scanned or backfilled automatically. Upgrade Taskix, this plugin, and Agentix together. See [conversation capture](https://github.com/tenfyzhong/agentix/blob/main/docs/task-board.md#job-conversation-records).

Before the lifecycle write, the packaged `discussion.mjs` helper classifies pending turns against the final delivery target. It uses the same opt-in Jev settings below, sends full candidate messages rather than excerpts, and never writes task state. The current turn is always included. Disabled Jev, uncertainty, errors, incomplete candidate pages, more than 24,000 bytes of context, or the eight-second deadline defer to the agent, which reads the original pending turns. The helper rechecks candidate and target revisions and returns guarded write arguments. Creation also binds the classified title, goal, and current prompt with a fingerprint. A changed target requires reassessment. This selection is separate from prompt-time Job routing and its metrics.

## Bundled entrypoints

| Host | Plugin configuration | Lifecycle entrypoint |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json` | Explicit `hooks/hooks.json` and `hooks/codex.json` |
| Claude Code | `.claude-plugin/plugin.json` | Default `hooks/hooks.json` plus manifest `hooks/claude.json` |
| Pi | `package.json` → `pi.extensions` | `extensions/pi.ts` |
| OMP | `package.json` → `omp.extensions` | `extensions/omp.ts` |

Codex and Claude share the common lifecycle hooks. Codex explicitly loads the shared file and a separate Interrupt hook; its manifest replaces default discovery, so each hook loads once. Claude merges default discovery of the shared file with its manifest-selected `hooks/claude.json`, which only adds PostToolUseFailure. Do not repeat the shared file in Claude’s manifest. Pi and OMP each select exactly one extension, so neither loads the other host's entrypoint. Pi declares the shared `skills/` directory in its manifest. OMP installs the marketplace plugin and discovers its shared `skills/` directory. The npm `files` list includes all four host manifests, hooks, extensions, runtime, skills, TaskNotes settings and setup guide, and this guide.

## Optional Jev routing

Jev can classify each user prompt in the host before the coding agent runs. It is
opt-in; the existing Agent-driven workflow remains the default. Set these
variables in the environment inherited by the host and its hook subprocesses:

```fish
set -gx TASKIX_JEV_ENABLED true
set -gx TASKIX_JEV_URL https://api.typesafe.ai/v1/systemone
set -gx TASKIX_JEV_API_KEY YOUR_API_KEY
# Optional:
set -gx TASKIX_JEV_MODEL jev-latest
set -gx TASKIX_JEV_MIN_CONFIDENCE 0.65
```

| Variable | Behavior |
| --- | --- |
| `TASKIX_JEV_ENABLED` | Only `true` or `1` enables routing, case-insensitively. Default: disabled. |
| `TASKIX_JEV_URL` | Full HTTP(S) evaluation endpoint, including `/v1/systemone` for TypeSafe. No URL is assumed. |
| `TASKIX_JEV_API_KEY` | Sent as `Authorization: Bearer ...`; never included in model context or routing receipts. |
| `TASKIX_JEV_MODEL` | Defaults to `jev-latest`; may pin a provider-supported Jev version. |
| `TASKIX_JEV_MIN_CONFIDENCE` | Defaults to `0.65`, valid range `0.5`–`1`. Both confidence and selected-option probability must meet it; the winning probability must also exceed the runner-up by at least `0.2`. |

Disabled routing, blank URL/key, or invalid configuration uses the original
workflow without a Jev request. Network/HTTP/JSON errors, an eight-second routing
deadline shared by prompt preparation (including hook heartbeat, context and transcript reads),
candidate lookup and HTTP/body reading, low confidence, conflicting options, missing context, or stale selected
Job revisions defer to the current Agent. They never mean “create a new Job.”
We recommend the built-in default of `TASKIX_JEV_MIN_CONFIDENCE=0.65`.
An explicit environment value overrides it. See the [confidence threshold guide](https://github.com/tenfyzhong/agentix/blob/main/docs/jev-confidence-thresholds.md)
for the comparison table, provisional error labels, and configuration guidance.
Confidence is a provider statistic, not a measured correctness guarantee;
review representative conversations when tuning the threshold.

The host sends the current prompt, current assignment IDs, recent conversation
excerpts, current-Project unarchived ACTIVE/PENDING_REVIEW Job facts,
Task states and waiting reasons, and available Inbox requirements to the configured
endpoint. It does not send task lease tokens or separately collect reasoning, tool output
or source file contents. The current prompt stays verbatim. For each Job, two distinct historical messages are sent in addition to
references to the shared recent session dialogue; old user
messages are limited to 512 UTF-8 bytes and assistant replies to 256 bytes, including
an explicit `[excerpt]` marker. Repeated current prompts and history already present
in the session excerpt are omitted. Completed/cancelled Task details, filesystem
paths, session identities, and revision metadata stay local. Original Job requirements
and waiting reasons remain available for matching. Missing evidence must select
`uncertain`; excerpts are not complete conversation records. Prompt-time transcript
reads are limited to 256 KiB; unreadable or oversized current-turn history defers
to the Agent. When only older optional turns exceed the read budget, routing
retains already parsed visible messages, even if their turn boundary is outside
the window. Incomplete JSON records are discarded.
Abort and I/O errors still defer. Stop-time conversation recording retains its existing behavior. More than
32 candidate Jobs, 256 unfinished candidate Tasks, or 32 Inbox entries, or a full serialized
request exceeding 30,000 UTF-8 bytes (including model, state, and questions),
defers to the Agent without silently dropping candidates. Taskix state remains
local and authoritative. The context ceiling is 32k tokens for state plus its longest question,
matching the provider limit. The 30,000-byte total-request guard
is a conservative policy below that ceiling, leaving service framing headroom. It is not an
exact Jev token count and does not use a characters/4 estimate. Over-budget prompts
return to the main Agent without HTTP; they are never silently cut to fit. Prompt preparation uses one `taskix routing snapshot --session SESSION_ID`
process to import Inbox edits, renew ownership, and read bounded assignment, Inbox,
and candidate facts. Candidate Jobs and Tasks use two SQL queries in one read
transaction. A selected Job is checked with
`taskix routing revision JOB_ID`, which excludes prompt and conversation bodies.
This makes one CLI call per prompt, or two when revalidating a selected Job.
`taskix routing candidates PROJECT_ID` remains available for candidate diagnostics;
candidate and revision commands require exact IDs. SQL bounds Job title/prompt/goal to 300/4,000/2,000
characters, each recent message to 2,000, and Task title/reason to 300/1,000;
Truncated requirements or waiting reasons mark the snapshot incomplete and defer classification. Historical messages carry an explicit excerpt marker; long historical replies alone do not invalidate candidate coverage. DONE and CANCELLED Tasks do not consume the unfinished-task budget.
Enable Jev with a matching taskix CLI; older CLIs defer to the original Agent workflow.
See the [TypeSafe API](https://docs.typesafe.ai/api) for
the request and response contract.
The repository document `docs/task-routing-performance.md` contains repeatable
local benchmarks, process counts, and measurement boundaries.

Routes are `followup`, `resume`, `new_job`, and `discussion`, with an explicit
uncertainty option. Waiting Tasks are found through ACTIVE Project Jobs even when
`context.previous_job` is empty. A confident result contains only the selected
Job/Tasks and matched Inbox IDs. Model-visible excerpts include up to 2,000 prompt
characters, 1,000 Goal characters, the last two messages (1,000 characters each),
and up to eight unfinished Tasks with a total Task count. Truncation is explicit;
the Agent can fetch complete details with `job show` / `task list` / `task show`. An existing owned assignment cannot be silently
redirected. The Agent still issues guarded taskix lifecycle commands, including
`job followup`; Jev never writes state, approves work, claims tasks, or starts Inbox
intake. The original prompt, review policy and dependency rules remain in force.

Codex/Claude run this at `UserPromptSubmit`; Pi/OMP use `before_agent_start`. With
valid Jev configuration, SessionStart gives a short session notice. After prompt
routing, Codex/Claude suppress repeated workflow/Inbox injection during tool calls,
including when the Agent received a fallback at prompt entry. Cancellation notices
and lease heartbeats remain active. Routed tool hooks reuse the heartbeat's live
cancellation facts and skip the additional `context` process. Older CLI versions
without cancellation facts retain the context check. Disabled or incomplete Jev
configuration makes `UserPromptSubmit` return without a taskix CLI call.
A private, expiring receipt in the OS temporary
directory coordinates separate hook processes; it contains only a turn identifier
and expiry. Session start/end, interruption, Stop, and the next prompt clear it.
A missing/unreadable receipt or a different Codex turn uses the legacy tool notice.
Hosts must load the new prompt hook; previously loaded plugin code is not hot-replaced.

Jev remains the first routing step. When it is uncertain or fails, the hook returns
bounded summaries directly to the current main Agent; no classifier subagent or
private delegation snapshot is used. The routing fallback is capped at 12,000 UTF-16 code units, excluding host discussion
and cancellation notices, and includes assignment and candidate references.
It is not complete evidence: the Agent reads omitted facts through `taskix context`
and `job/task show` before selecting ownership or Inbox matches. Existing assignment,
claims, execution plans, dependencies and review policy still apply.

Before fallback followup, the main Agent reads the selected Job's current revision
and supplies `--expect-revision`. Jev success instructions also bind the observed
revision. A conflict requires reassessment, never dropping the guard. Disabled
routing keeps the original context format. Install the updated plugin and reload
the host to replace previously loaded delegation instructions.

## Runtime boundaries

| Module | Responsibility |
| --- | --- |
| `jev.mjs` | Provider requests, confidence checks and semantic routing advice |
| `routing-context.mjs` | Pure, bounded rendering of fallback hints; no I/O or task writes |
| `runtime.mjs` | Shared Codex/Claude/Pi/OMP orchestration, deadlines, receipts and heartbeats |
| `taskix-cli.mjs` | Shell-free CLI execution, host identity arguments and error handling |
| `discussion.mjs` | Discussion selection and its standalone command-line entrypoint |

The runtime re-exports `buildArgs` and `runTaskix` for existing callers. The standalone
discussion helper loads the CLI adapter directly, avoiding a runtime/discussion
import cycle. Its entrypoint recognizes canonical and symlinked installation paths.

Fallback instructions refer to the workflow skill instead of repeating its full
policy. Identity hints have priority over body excerpts. The renderer considers at
most 32 Job and 32 Inbox entries, serializes each fragment once, and accounts for
JSON escaping and separators before appending it. Counts and `candidates_complete`
make omitted evidence explicit. A discussion-only prompt needs no lifecycle write;
Jev confidence thresholds and task ownership rules are unchanged.

## Optional Jev statistics

The plugin only collects and appends observations; taskix owns reports and human
labels. The [versioned storage protocol](metrics-schema.md) defines their shared
contract. Unknown or nonempty unversioned databases are rejected without migration;
preserve old development statistics and choose a fresh database path for v1.

Statistics are disabled by default, independently of Jev routing. Enable them in
its host process environment before starting/restarting the host:

```fish
set -gx TASKIX_JEV_METRICS_ENABLED true
# Optional: use an absolute path to a separate statistics database.
set -gx TASKIX_JEV_METRICS_DB "$HOME/.local/state/taskix/jev-metrics.sqlite"
```

Only `true` (case insensitive) or `1` enables writes. Unset/false skips metric
collection, SQLite loading, directory creation and database access. Jev itself must
also be enabled with valid configuration. Recording starts with subsequent prompts;
there is no historical backfill. Tool heartbeats do not
write routing statistics. The default database is
`$XDG_STATE_HOME/taskix/jev-metrics.sqlite`, or
`~/.local/state/taskix/jev-metrics.sqlite` when XDG_STATE_HOME is unset. It is separate
from the Taskix task database. Newly created directories/files use 0700/0600 modes.

Each prompt records a generated request ID, timestamp, host session/turn when
available, Project ID, model, configured threshold, preparation duration, whether
HTTP was attempted, whether the route was accepted, and fallback reason. Each
route/Inbox answer records its known choice, Inbox ID, confidence, selected-option
probability, runner-up margin, validation status and rejection issue. It stores no
prompt/history/Inbox bodies, API keys, endpoint URLs, leases or raw model response.
Metrics stay outside injected Agent context.

Use the taskix CLI from any directory; no task database initialization is required:

```sh
taskix routing metrics report
taskix routing metrics list --limit 50
taskix routing metrics label REQUEST_ID correct
taskix routing metrics label REQUEST_ID incorrect
```

Add `--json` for the standard taskix response envelope. Reports display text
tables by default; list/label use the standard human-readable JSON display.
Queries use the same `TASKIX_JEV_METRICS_DB` setting; reading an absent database
fails instead of creating one. `list` shows the latest 50 requests and their
question scores; use session/turn/time to locate the corresponding conversation.
`report` groups adoption and reviewed accuracy by model and configured threshold,
lists fallback/question issues, and compares all-question score gates at
0.85/0.90/0.95. `response_quality` gives valid-response and low-confidence-response
counts: divide the latter by the former for the fraction of valid responses with
at least one low score. A failed Inbox answer can cause the entire prompt to defer.

Score-gate passes are not predicted adoption: later assignment/revision checks
might reject, and missing/invalid/uncertain answers cannot be fixed by lowering a
threshold. `reviewed_accuracy` measures only manually labeled accepted requests;
without labels it is null, not 100%. Label the correctness of the complete proposed
route and Inbox selection, not agreement with another model. Review both accepted
and deferred samples before changing thresholds.

Writing uses one transaction per prompt in a dedicated worker and closes the connection afterward. The worker receives only sanitized metrics, with an empty environment; no API key or prompt is transferred. SQLite lock waits use a 25 ms busy timeout. The parent waits at most 250 ms for worker startup and persistence, then requests termination and continues without awaiting OS cleanup. This bounds parent waiting under normal event-loop scheduling, not physical disk completion. Failures skip the sample with a generic stderr notice and never alter routing. The report labels `duration_ms` as `PREP_MS`: preparation and Jev time, excluding statistics writing and native subagent work. Reports describe successfully stored
samples, so dropped writes can bias them. No automatic retention/deletion is
performed; disable collection when finished. The query tool adds no model calls.

## Prerequisites and activation

Install Node.js 24+ and put `taskix` on PATH. Initialize taskix with your chosen document directory before enabling the plugin. Set `TASKIX_CONFIG` if its configuration is not in the default location.

### Codex: marketplace

The repository provides the `agentix` marketplace in `.agents/plugins/marketplace.json`. Add the GitHub repository, then install the plugin:

```sh
codex plugin marketplace add tenfyzhong/agentix --ref main
codex plugin add taskix-manager@agentix
codex plugin list
```

Start a new Codex thread after installation, then use `/hooks` to review and trust the bundled hooks. Installation does not bypass hook trust. See [Codex marketplace commands](https://learn.chatgpt.com/docs/developer-commands#codex-plugin-marketplace) and [plugin commands](https://learn.chatgpt.com/docs/developer-commands#codex-plugin).

### Claude Code: marketplace

The repository also provides `.claude-plugin/marketplace.json` with the same marketplace and plugin names. Run these commands in your terminal:

```sh
claude plugin marketplace add tenfyzhong/agentix
claude plugin install taskix-manager@agentix
claude plugin list
```

Inside Claude Code, the equivalent commands start with `/plugin`. Reload plugins if the host requests it, and review the discovered hooks with `/hooks`. See [Claude Code marketplace installation](https://code.claude.com/docs/en/plugin-marketplaces#manage-marketplaces-from-the-cli).

Both hosts fetch the marketplace catalogs from GitHub. Source checkouts contain both catalogs; `taskix-*` release archives provide the plugin directory, not a marketplace root. The `agentix-*` archives do not include the plugin. Both hosts load the shared `hooks/hooks.json`; Codex additionally loads `hooks/codex.json`, and Claude loads `hooks/claude.json`. No per-project hook files need to be copied.

### Hook failure diagnostics

Failed command hooks exit with code 1 and print the underlying error to stderr,
including the Taskix command words when a CLI call fails. Taskix JSON error
messages take precedence over subprocess stderr. Hosts may display only a generic
hook failure, so the entrypoint also appends a JSON line to
`$XDG_STATE_HOME/taskix/hooks.jsonl`, or `~/.local/state/taskix/hooks.jsonl` when
`XDG_STATE_HOME` is unset. Stderr includes the log path after a successful write.

Each entry records the timestamp, hook event, session ID, working directory,
command words, available exit code or spawn error code/signal, and error message.
The logger does not copy the event payload, transcript, environment, or complete
CLI argument list. Error messages can contain local paths or subprocess-provided
details; newly created log files use owner-only permissions. The file is
append-only and can be removed or rotated when needed. Log write failures are
reported to stderr without replacing the original failure. Successful hooks do
not write this error log. Failures before the entrypoint loads, or processes
forcibly killed by the host timeout, cannot be captured by this handler.

### Pi: install

Install the package directly from GitHub:

```sh
pi install git:github.com/tenfyzhong/agentix
```

Pi manages the Git checkout and installs npm dependencies automatically. The repository-root `package.json` selects the plugin's Pi extension and shared skills; its npm workspace installs the plugin's runtime dependencies. Restart or reload Pi after installation. See [Pi package installation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/packages.md#install-and-manage).

Keep only one Agentix installation enabled in Pi. Installing the GitHub package while a local checkout is still registered can produce `Tool "taskix" conflicts with ...`. Remove the old checkout with `pi remove /absolute/path/to/old/checkout`, then restart Pi. This removes its registration without deleting the source files.

Project-local installations created with `pi install -l` must be removed with `pi remove /absolute/path/to/old/checkout -l` in that project.

### OMP: install

Install the extension package directly from GitHub:

```sh
omp plugin marketplace add tenfyzhong/agentix
omp plugin install taskix-manager@agentix
omp plugin install agentix-bridge@agentix
```

OMP installs the plugins from the same marketplace used by Claude. The marketplace selects `plugins/taskix-manager/` and `plugins/agentix-bridge/`; each plugin selects its OMP extension through `omp.extensions`. OMP discovers `plugins/taskix-manager/skills/`, so all hosts use the same workflow and reference files. No repository-root skill copies are required. For local development, `make dev-test` registers the current checkout as the marketplace. Restart OMP after installation.

If `skill://taskix-manager` reports `Unknown skill` while the taskix tool is available, remove the old root package with `omp plugin uninstall agentix-plugins`, install the marketplace plugins above, and restart OMP. In a new session, check `/status` for the skill and read `skill://taskix-manager` and `skill://taskix-manager/references/commands.md`. Older repository packages placed skills only below `plugins/taskix-manager/`, outside OMP's discovery root.

An npm-installed copy uses `npm install --ignore-scripts` if dependencies need reinstalling: npm does not ship `package-lock.json`. Source and release copies include the lockfile and can use `npm ci`. The separate Obsidian skill is still required when an agent edits Obsidian Plan/Notes bodies.

## Document layout

Obsidian uses `Dashboard.base` for a compact project table showing clickable names, status, and recent activity, sorted newest first. Formula columns keep the displayed state read-only. Archived projects are hidden. Sync publishes this Base before removing the old generated `Dashboard.md`; unrelated same-name files are preserved.

Generated Markdown documents use readable names, YAML frontmatter and type tags; `Dashboard.base` uses native Base YAML with a generated-file comment. Job and Task note filenames include a stable `YYMMDD-seq-` prefix, with separate daily counters per project and type; taskix assigns these automatically. Each Task has one TaskNotes-compatible note in the project’s `Tasks/` directory, even before planning. Frontmatter records task status and metadata; the body is freely organized by the agent. Task timestamps use the computer’s local time zone and `revision` is the sole document revision field. Plan commands update that body in place. Jobs directly link Task notes, while Board embeds a TaskNotes Base. Agents choose concise Job/Task names with `--name`. Unarchived Jobs are stored directly in `Jobs/`; archived Jobs are stored in `Jobs/Archived/`. Completed Tasks remain on the Board until their Job is archived. `taskix job delete JOB_ID` permanently deletes the Job and its Task notes; `taskix project delete PROJECT_ID` removes all project work and its entire generated project directory. Release active Task leases first; remove dependencies from surviving Jobs before deleting a Job. `sync` retries interrupted file cleanup. `AGENT_TASK_LANG=zh-CN` tells the skill to decompose tasks and author names, goals, Notes, and Plans in Chinese. Hooks and extensions expose this preference as `task_language` in agent context. Unset or blank defaults to English; other languages such as `ja` are supported by the agent. taskix has no language setting and renders fixed English labels. Configure the seven TaskNotes statuses from the [bundled TaskNotes guide](obsidian/README.md#configure-the-seven-statuses).

## Lifecycle behavior

The shared Skill uses `claim → Plan → start → execute/verify → done`. Claim reserves PLANNING before the agent drafts a Plan; Plan publication requires the current lease. Start checks the Plan and dependencies and switches to EXECUTING with the same token. Pi/OMP resolve unambiguous Task prefixes and automatically attach the lease to owned Task/Plan writes and an idempotency key to metadata mutations, including Job/Project deletion. Retrying the same delete call returns its committed result without deleting twice or adding duplicate events. Codex/Claude shell calls supply them explicitly.

| Trigger | Behavior |
| --- | --- |
| Codex/Claude `SessionStart` | Restore eligible Tasks to PLANNING with a new token; inject full context by default or a short notice with Jev enabled |
| Codex/Claude `UserPromptSubmit` | Optionally classify ownership with Jev and inject selected context or Agent fallback |
| Codex/Claude `PreToolUse`, `PostToolUse` | Renew leases and surface human cancellation facts |
| Codex/Claude `Stop` | Renew leases; leave Inbox work pending for explicit user input |
| Codex `Interrupt` | Block active Tasks owned by the interrupted session and release their leases |
| Claude `PostToolUseFailure` with `is_interrupt: true` | Release that session’s active leases; ordinary tool failures do nothing |
| Codex/Claude `SessionEnd` | Block active Tasks owned by the ending session and release their leases |
| Pi/OMP `session_start` | Restore eligible Tasks to PLANNING and start a one-minute heartbeat timer |
| Pi/OMP `before_agent_start` | Wait for pending cleanup, restart paused heartbeat, and inject current task facts; never reclaim implicitly |
| Pi `agent_end` + `agent_settled` | Release after an aborted run settles without automatic continuation |
| OMP `agent_end` | Release when the latest assistant response is aborted and `willContinue` is not true |
| Pi/OMP `session_shutdown` | Cancel the timer and in-flight heartbeat, then block active Tasks and release leases |

Hook commands resolve `CLAUDE_PLUGIN_ROOT` or `PLUGIN_ROOT` inside Node, without shell-specific variable expansion. Paths containing spaces or Unicode remain one filesystem path. Pi/OMP register a structured `taskix` tool that supplies session, executor, lease token, and idempotency identity.

Renewal and expiry apply during both planning and execution. Recovery does not require a Plan yet and never calls start automatically: the agent must repair/review the current Plan and explicitly start before continuing execution. Hooks never create or revise Plans. Registered Plan files are published through taskix, not overwritten directly by agents.

The shared `SessionEnd`, Codex `Interrupt`, and Claude `PostToolUseFailure` hooks request a three-second timeout; other command hooks allow 30 seconds. If shutdown times out or the process is killed, recovery falls back to the 15-minute lease expiry checked by subsequent task operations. Claude additionally applies a session-exit budget (1.5 seconds by default in current versions); a timeout in plugin hooks does not raise that budget. If cleanup needs more time, launch Claude with `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS=3000`. Codex/Claude have no periodic timer, so long tool calls or idle gaps can also expire a lease. Hooks never force task completion.

Codex emits `Interrupt` when an active main-thread turn is interrupted. The Task becomes system-blocked with reason `session interrupted`, its Plan is preserved, and its old lease token cannot write. Later heartbeats cannot revive that lease. Recovering released Inbox work requires an explicit user request and `inbox claim-next` to acquire a fresh Inbox lease; Stop never claims it. Existing Tasks still require claim and start. To continue in the same thread, claim again, review the Plan, and start; a later SessionStart can also restore eligible Tasks to PLANNING. Released leases no longer prevent Job deletion, though dependency checks still apply.

Exiting an idle CLI connected to a persistent app-server may only disconnect the client: it does not guarantee Interrupt or immediate SessionEnd. Force-killing the process can also skip hooks. After stopping the agent, explicit cleanup is available with `taskix hook session-end --session SESSION_ID`; `taskix hook interrupt --session SESSION_ID` applies the interrupted state. Otherwise expiry remains the fallback. See [Codex hook events](https://learn.chatgpt.com/docs/hooks).

Pi and OMP stop heartbeat timers on detected interruption before invoking cleanup, cancel an in-flight heartbeat without cancelling cleanup, and ignore queued ticks and lifecycle events from the previous session. Cleanup failure keeps renewal stopped and is reported through the host; repeated events can retry. A new prompt waits for pending cleanup before restarting heartbeat; the agent still needs to claim a released Task, review its Plan, and start. Normal replies and automatic continuations retain ownership. Pi requires the `agent_settled` extension event (present in Pi 0.84.4); OMP requires `agent_end.willContinue` to distinguish internal retries. These adapters detect aborted assistant results; aborts that produce no such result fall back to session shutdown or explicit cleanup. They do not intercept terminal keystrokes or process signals themselves.

Claude’s PostToolUseFailure adapter only acts on the boolean `is_interrupt: true`. It is best-effort coverage: Claude has no general Interrupt hook, Stop does not fire for user interruption, and cancelling a running tool does not necessarily emit PostToolUseFailure. Use SessionEnd for actual exit and `taskix hook interrupt --session SESSION_ID` for explicit cleanup after stopping work when no event is emitted. See [Claude hook events and shutdown budgets](https://code.claude.com/docs/en/hooks), [Pi extension lifecycle](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md), and [OMP extension events](https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/extensibility/shared-events.ts). Force-kill, crashes, and older hosts missing these events still rely on expiry.

When updating an existing local marketplace installation, rebuild/install taskix first, then refresh the plugin with `codex plugin add taskix-manager@agentix`. Codex CLI 0.153.4 refreshes the installed local copy even when the plugin version is unchanged. Start a new thread and use `/hooks` to review and trust the changed hook configuration, including Interrupt. Source edits alone do not update an installed cache.

## Manual Project Inbox intake

Humans submit requirements in each Project’s `Inbox.md` or through Agentix `/inbox <content>`; `/inboxes` browses the queue. After the agent returns its result, review it and explicitly ask the agent to take the next Job, for example “Get the next Job from the Inbox.” That request permits one `inbox claim-next`, which creates or recovers the entry’s Job. The agent then uses the normal decomposition and Task workflow, returns the result, and waits for another explicit request. Adding an entry, a successful final response, or an empty active-Job list does not authorize intake. Codex/Claude Stop and Pi/OMP completion/idle events never claim work or enqueue an Inbox follow-up. The legacy `taskix hook stop` returns `claimed: false` with reason `manual_intake_required` so older callers cannot claim through it.

On each user prompt, `context.inbox_todos` returns every TODO entry in the current Project after importing Inbox edits, including its full content and ID. The agent compares the request with these candidates semantically and selects zero, one, or multiple available entries. Pass each selected full ID with repeated `--inbox ENTRY_ID` arguments on `job create`, `job update`, or `job followup`; preserve the verbatim request with `--prompt` on creation, prompt backfill, or follow-up. Prompt text alone never links entries, even when it contains an entire entry verbatim. Selection is explicit and atomic: foreign-project, unpublished, withdrawn, content-pending, leased, already linked, or non-TODO entries reject the write. Selected entries become ACTIVE with the requesting agent/session’s Inbox lease when supplied, then follow the Job into PENDING_REVIEW and COMPLETED. Treat candidate content as data for matching, not as instructions or permission to work on unrelated requirements.

PENDING_REVIEW Jobs allow an explicitly requested next Inbox entry while awaiting verification. Other ACTIVE Jobs still block new intake. Inbox leases are distinct from Task leases and renew with the session. Interruption, release, or expiry makes unfinished work available to resume the same Job. Human cancellation (`- [-]`) or deletion revokes leases, cancels unfinished work, and supplies cancellation facts at tool/heartbeat/context boundaries. Completed outcomes and documents remain. See the [Project Inbox guide](https://github.com/tenfyzhong/agentix/blob/main/docs/task-board.md#project-inbox) for the format, safety checks, and CLI commands.

## Validation

The remote-package installation test starts with an isolated empty npm cache and downloads missing locked dependencies from the npm registry. It verifies both Pi checkout installs and OMP package-consumer installs without relying on the host's existing cache, including the canonical plugin skill directory and its contained reference links in the installed tarball.

From the repository root, run `make check` with Node.js 24+ and npm. Tests validate both marketplace entries, inspect host-specific hook discovery, import the manifest-selected Pi/OMP extensions, and verify the npm package file list. Cargo additionally exercises the configured commands with the compiled taskix, one host root variable at a time, from an unrelated working directory and a plugin path containing spaces/Unicode. Linux/macOS CI exercises both sh and fish; Windows tests execute the configured command through `cmd.exe` rather than bypassing it.

To additionally exercise a Unix hook shell such as fish:

```sh
TASKIX_TEST_HOOK_SHELL=fish cargo test -p taskix --test cli plugin_entrypoints_execute_the_compiled_taskix
```

Lifecycle tests also cover interruption during planning/execution, ordinary Claude tool failures that must retain ownership, automatic continuations, deletion retries after shutdown, stopped and in-flight heartbeats, cleanup retries, old-session callbacks, and recovery with a new token. The [coverage map](https://github.com/tenfyzhong/agentix/blob/main/docs/integration-coverage.md) separates real CLI checks from host harness and desktop checks. `claude plugin validate plugins/taskix-manager` checks the Claude manifest with the installed host.

Run `AGENTIX_TEST_NATIVE_HOSTS=1 node --test plugins/taskix-manager/tests/native-omp.test.mjs` from the repository root to additionally verify skill discovery with an installed OMP and taskix. This starts an isolated RPC session with the marketplace plugins and checks the host's skill list through the bridge, then uses its native Read tool on `skill://taskix-manager` and `skill://taskix-manager/references/commands.md`, without sending a model prompt or using the user's task database.

These tests do not install the plugin in a user's host or invoke a live model. Native host loading, trust policy, and credentialed IM behavior remain separate acceptance checks.

## Obsidian setup

See the [TaskNotes setup guide](obsidian/README.md) for task identification, seven English statuses, migration, and usage. [tasknotes-settings.json](obsidian/tasknotes-settings.json) supplies the settings subset; merge it with existing vault settings. TaskNotes provides status colors. The plugin does not automatically change vault appearance.

## Obsidian status editing and Job review

Run `taskix obsidian setup` to install TaskNotes and the bundled desktop Taskix Sync plugin. Board contains a pastel Job status board above the Task board. Saved status edits are submitted through taskix; rejected changes are restored with a notification. See [Obsidian setup and supported edits](obsidian/README.md#status-edits).

When all non-cancelled Tasks are DONE and at least one exists, the default `review_policy: required` submits the Job to PENDING_REVIEW. Investigation-only, document-only, and simple git commit/push Jobs use `job create/update --review-policy none` and complete directly without separate human approval. Mixed Jobs containing code changes retain `required`. Verification passes with `job approve`, or fails with `job reject --reason` to return the Job to ACTIVE while preserving Task outcomes. Use `job submit` to resubmit ready work explicitly. Update all taskix and Agentix database writers together.

Git and gh delivery requests such as `git commit`, `git push`, `gh pr create`, and `gh pr edit` also supplement a pending Job when they concern its changes, even if the prompt only says "commit" or "create a PR". Resolve that ownership before creating a Job or choosing a review policy. Use the conversation and repository/worktree evidence to confirm the delivery; if `context.previous_job` is absent, inspect the current Project with `job list --pending-review` rather than assuming the request is independent. Tool names alone do not establish relevance. For a match, run `job followup` before delivery work, then add new Tasks to the same ACTIVE Job with the old Tasks as dependencies. Preserve the whole Job's review policy: a Git-only supplement to an implementation Job still requires review. Only independent operational Jobs use `none`; unrelated requests and requests after COMPLETED get new Jobs.

A new prompt supplementing a PENDING_REVIEW Job reuses it through `job followup JOB_ID --prompt 'Verbatim supplementary request'`. Context and hooks expose `previous_job` as a candidate for the agent to assess; they do not reopen every Job automatically. Follow-up returns the Job to ACTIVE, preserves old Tasks, and makes each newly added Task depend on the snapshot of all old Tasks. Unsatisfied prerequisites still block execution. Independent requests and requests after COMPLETED get new Jobs. The original Prompt is preserved; Conversation records successive `Turn N` sections, each with User input and Agent output.

### Short follow-up references

Routing includes up to eight recent dialogue messages (8,192 serialized bytes),
with 1,024-byte user and 1,536-byte assistant excerpts that retain the beginning
and end. Progress updates retain the latest preceding user request even when
eight assistant updates would otherwise displace it. Codex/Claude read up to eight turns within the existing 256 KiB limit;
Pi/OMP retain eight messages. The complete request remains capped at 30,000 UTF-8 bytes. The latest user and assistant messages
also have focused 4,096/8,192-byte excerpts with their matching source Jobs.
Leading host plugin catalogs and internal continuation wrappers are removed
before extracting visible user messages, while any following real request remains.
The previous Job is an explicit hint, not an automatic ownership decision.

Jev answers separate intent and ownership questions in one HTTP request. Both
must pass the existing score gates. A question about an existing delivery returns
`action: discussion` with that `job_id`. The
main Agent receives that Job context without reopening it or creating a Task.
An implementation request referring to the same conversation uses the existing
revision-guarded resume/followup workflow. Discussion drafts are still attached
when work resumes; routing alone does not mutate their ownership. Ambiguity still
falls back to the main Agent. No threshold or statistics configuration is changed.

When a candidate Job contains the same visible dialogue as the recent context,
`recent_conversation_indices` preserve that source relationship instead of
silently discarding duplicated text. References are zero-based indices into the
final bounded dialogue, recomputed after trimming; multiple matching Jobs remain
visible. They are evidence for the model, not an automatic ownership decision.
A same-session boolean supplies additional evidence without exposing session IDs
or automatically assigning ownership. Route options include bounded Job titles, and instructions distinguish accepting
an earlier implementation suggestion from asking another question about it.
