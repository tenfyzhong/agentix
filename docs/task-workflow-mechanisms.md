# Task Decomposition, Skills, and Hooks

This document explains how the task-management components work together: who breaks down requirements, who decides when work starts or finishes, who maintains session state, and which operations are protected when multiple agents work concurrently. For command arguments and installation instructions, see the [task board guide](task-board.md) and [plugin guide](../plugins/agent-task-manager/README.md).

First decompose the requirement into a Task DAG, create or resolve every node’s Task ID, and configure each edge A → B with `task depend B_ID A_ID`. Verify the stored dependencies against the DAG before implementation. Each Task already has a note with managed frontmatter at this point. When taking up a Task, follow `claim → publish Plan in its note → start → execute and verify → done`. The agent makes decisions using the Skill, hooks respond to host events, and taskcli validates and persists task state.

## 1. Responsibilities

| Component | Responsibilities | Outside its scope |
| --- | --- | --- |
| Agent | Understand requirements, decompose work, claim Tasks, write Plans, execute, and verify acceptance | A natural-language statement such as "finished" does not change database state |
| Skill | Provide the agent with a workflow, command guidance, and document rules | Does not run continuously, acquire locks, execute tests, or enforce compliance |
| Hook / Extension | Handle session events, inject task context, renew leases, and handle exits and resumption | Does not decompose work, generate Plans, or automatically call start or done |
| taskcli / agentix-task | Validate state transitions, ownership, dependencies, revisions, and idempotency; persist changes | Does not judge business correctness or run acceptance commands from a Plan |
| SQLite | Store Project, Job, Task, and Plan metadata, leases, and events | Does not store complete Plan bodies or isolate code workspaces |
| Markdown / Obsidian files | Store Plan bodies and editable Goal/Notes sections; display generated boards | Are not an input source for task-state changes; no file watcher imports status edits |

The Skill defines working instructions, hooks adapt host events, and taskcli provides validated task operations. None replaces the others.

## 2. Decomposing Work

### 2.1 Project, Job, Task, and Plan boundaries

- **Project: a long-lived project.** Worktrees of the same Git repository share one Project. Non-Git work can use a stable directory.
- **Job: an independently acceptable requirement.** New requirements for the same project get new Jobs instead of being appended indefinitely to a completed Job.
- **Task: work that one agent can claim and deliver.** Each Task should have a clear result, scope, and verification method.
- **Plan: the current Task's execution approach.** Write it after claiming the Task; publish changes through `plan revise` to update the same file and advance the Task revision.

A Job's `goal` records the overall objective and acceptance conditions. Tasks currently have no separate structured acceptance field. Put the detailed scope, test approach, and risks in the Plan; delivery notes can go in the Job's Notes section.

### 2.2 Split by deliverables, not individual actions

For example, a Job to export a task list could be organized as follows:

```text
Project: agentix
└── Job: Export the task list
    ├── Task A: Implement the export data interface, including unit tests
    ├── Task B: Implement an independent CSV encoder, including unit tests
    └── Task C: Integrate the CLI and add end-to-end acceptance checks
        Dependencies: A, B
```

If the interface contract is already clear, A and B can run concurrently. C can be claimed and planned early, but it cannot start until both A and B are DONE. If B actually depends on A's output, declare that dependency rather than omitting it to increase concurrency.

Check the following when decomposing work:

1. Does each Task produce a verifiable result, rather than merely describe an action such as "read the code" or "edit files"?
2. Are its inputs clear, and does it depend on another Task's output?
3. Can it be completed in an independent worktree? If Tasks must change the same interface or configuration, should they first agree on a contract, declare dependencies, or assign an integration Task?
4. Does each behavioral Task include its own TDD cycle: a failing test, the smallest implementation, and passing validation?
5. Are all known initial Tasks and their dependency edges registered, with generated Task notes available, before implementation starts? This makes the scope visible and prevents premature Job completion. Prepare each detailed Plan when taking up that Task, after claiming it.

Do not split "write the failing test" and "implement the behavior" into independently executable Tasks that can bypass one another. Additional cross-module acceptance checks can be a downstream Task, but they do not replace each behavioral Task's own tests.

Manage dependencies with `task depend` and `task undepend`. They must stay within one Project, may cross Jobs, and must not form cycles. Dependencies cannot change after execution first starts; claim alone does not set the first execution timestamp.

Each Task note exports the prerequisite Task IDs in `dependencies`, including an explicit empty list when none exist. These properties come from SQLite and are restored by sync; Plan frontmatter cannot override them. `task start` checks the database and requires every prerequisite to be DONE. Use the completed outputs when preparing the dependent Task's Plan.

### 2.3 Who decomposes work, and how duplication is avoided

The primary agent currently creates Jobs and Tasks using the Skill, or the user assigns existing work. Hooks do not decompose requirements, and SQLite does not generate a task graph from natural language.

Use one coordinator to manage a Job's initial decomposition at a given stage, while execution agents claim existing Tasks. This is a coordination convention, not a Job-level planning lock provided by the system:

- `claim` protects an existing Task, not the decision about which Tasks to create.
- Two planners can submit separate requests that create semantically identical Tasks; the system does not automatically deduplicate them.
- An idempotency key prevents duplicate records when retrying the same request. It cannot recognize that two different requests describe the same requirement.

Exclusive ownership of one Task and one-time decomposition of a requirement are different problems. A future Team layer must define its coordinator and task-graph update process separately.

## 3. Why Claim Comes Before the Plan

If agents write a Plan before claiming the Task, two agents can plan and write files for the same unclaimed Task before discovering that only one can own it. A file-write lock serializes writes; it does not decide which agent should do the planning.

The workflow establishes ownership before allowing Plan publication:

```mermaid
sequenceDiagram
    participant A as Agent
    participant C as taskcli / Service
    participant D as SQLite
    participant F as Document directory
    A->>C: claim(task, executor, session)
    C->>D: Validate and create a lease in a short transaction
    D-->>A: PLANNING + lease token
    A->>A: Write the Plan using the Skill
    A->>C: plan create/revise + session/token
    C->>F: Acquire output lock, validate ownership, and write Plan
    C->>D: Revalidate and register the version in a transaction
    A->>C: start + session/token
    C->>C: Validate Plan file, dependencies, and lease
    C->>D: Switch to EXECUTING; retain token
    A->>A: Implement, test, and verify acceptance
    A->>C: done + session/token
    C->>D: Mark DONE and release the lease
```

The board still has seven status columns. PLANNING and EXECUTING are phases within IN_PROGRESS, not additional columns.

| Operation | Preconditions | Result |
| --- | --- | --- |
| claim | Task is claimable; neither the Task nor the executor/session pair has a lease | Enter IN_PROGRESS / PLANNING and obtain a new token; no existing Plan or completed dependencies required |
| plan create/revise | Hold the Task's valid session/token | Write and register a Plan version without starting execution |
| start | Be in PLANNING with a valid lease, a nonblank current Plan file, and all dependencies DONE | Enter EXECUTING with the same token; set `started_at` on the first execution start |
| done | Be in EXECUTING with a valid lease | Enter DONE, clear the phase and lease, and check Job completion |
| block / wait / fail / release | Satisfy the applicable transition rules; supply current ownership when leased | Record the reason and release the lease; release enters BLOCKED |

Validation of `done` establishes that the state and ownership are valid. **It does not prove that acceptance checks passed.** The agent must follow the Skill to check that tests ran and results meet the user's requirements, with user or independent review when needed.

A Job becomes COMPLETED only when it has at least one non-CANCELLED Task and every such Task is DONE. Cancelling every Task does not count as delivery.

## 4. How the Skill Works

The plugin's [agent-task-manager Skill](../plugins/agent-task-manager/skills/agent-task-manager/SKILL.md) is an instruction file for the model, not a background process. Once the host loads the plugin's skills directory, the agent uses it when relevant. Codex/Claude session-start hooks also remind the agent to use the Skill, but that reminder does not guarantee that the model has read or followed every instruction.

The Skill guides the agent to:

1. Read `taskcli context --session <actual-host-session-id> --json` first and prioritize continuing an existing Job/Task.
2. Decide whether the request needs durable tracking; do not create a Job for every short question.
3. Discover or register the Project, create or reuse a Job, and decompose Tasks and dependencies.
4. Draft and publish the Plan only after claim succeeds; do not keep writing that Task's Plan after a claim conflict.
5. Execute only after start succeeds, and renew the lease during both planning and execution.
6. Call done only after acceptance checks pass; use the appropriate command and a concrete reason for blockers, user decisions, or failures.

This creates two kinds of checks:

- **Agent judgment and working instructions:** sound decomposition, an adequate Plan, adherence to TDD, and actual acceptance verification.
- **Programmatic validation:** a valid lease, legal state transitions, DONE dependencies, an existing nonblank Plan file, and revision checks.

The program does not understand the Plan's meaning and cannot prevent arbitrary code-file edits before start. The Skill and code-workspace isolation remain necessary.

### Context is not complete shared memory

`context` returns facts such as Project/Job/Task IDs, Task status and phase, leases, the current Plan path, and document configuration. It does not include the full session history, a Job-wide shared memory, or the Plan body. The agent must additionally call `job show`, `plan show`, or read the relevant documents.

A session query may return empty task fields when there is no active Task. It does not automatically create a Job or claim new work.

## 5. How Hooks and Extensions Work

This document uses "hook layer" for both host integration mechanisms: Codex/Claude command hooks and Pi/OMP in-process extension callbacks. Both ultimately call the same taskcli.

### 5.1 Codex / Claude: command hooks

The repository provides [hooks/hooks.json](../plugins/agent-task-manager/hooks/hooks.json). Its commands run the Node entrypoint [hooks/run.mjs](../plugins/agent-task-manager/hooks/run.mjs), which reads host event JSON from stdin and passes it to the shared runtime.

| Configured host event | Commands | Plugin behavior |
| --- | --- | --- |
| SessionStart | `taskcli hook session-start`, then `taskcli context` | Attempt to resume eligible Tasks; return the actual session ID, task facts, and a Skill reminder as additional context |
| PreToolUse | `taskcli hook heartbeat` | Renew the session's active leases |
| PostToolUse | `taskcli hook heartbeat` | Renew the session's active leases |
| Stop | `taskcli hook heartbeat` | Renew only; ending a response turn is not Task completion or a session exit |
| SessionEnd | `taskcli hook session-end` | Mark the session's in-progress Tasks as system BLOCKED and release their leases |

Command hooks do not stay running or start a heartbeat daemon. There is no periodic heartbeat without tool events; a single long tool call or an idle gap exceeding 15 minutes can still expire the lease. A PostToolUse heartbeat after expiry cannot revive the old lease; session resumption or a new claim is required.

PreToolUse currently only renews leases. It does not check whether the next tool will bypass taskcli to write a Plan, and it is not a general file-write interceptor. The entrypoint returns an error when a hook fails; how the host displays or handles that error depends on its runtime behavior. A failed hook must not be treated as a successful renewal.

These descriptions cover the repository's configuration and implementation, not verified live loading in every host version. The host must load the plugin correctly and satisfy its hook-enablement and trust requirements. Installation entrypoints are documented in the plugin guide.

### 5.2 Pi / OMP: extension callbacks and a structured tool

Both [pi.ts](../plugins/agent-task-manager/extensions/pi.ts) and [omp.ts](../plugins/agent-task-manager/extensions/omp.ts) call the shared `registerExtension`. Each host selects its entrypoint through its package configuration.

| Callback or tool | Plugin behavior |
| --- | --- |
| session_start | Resume eligible Tasks and start a once-per-minute heartbeat; on a session switch, attempt to release the previous session's task ownership through session-end |
| before_agent_start | Query context and inject it as facts, not instructions |
| session_shutdown | Stop the heartbeat timer and run session-end |
| taskcli tool | Accept an array of argument strings, invoke the actual taskcli process, and return JSON results or errors |

For example, the agent supplies:

```json
{
  "args": ["task", "start", "task_ID"]
}
```

The extension supplies the session and executor from the actual host context and queries the current Task's lease. It automatically attaches the token when the full Task ID in the arguments matches the Task ID in context; do not assume short IDs trigger this behavior. Writes also receive an idempotency key derived from the host, session, and tool call ID.

The tool invokes a subprocess with an argument array, without interpolating agent-provided content into a shell command. The argument array cannot override managed identity parameters. Ordinary Codex/Claude shell calls do not receive this automatic argument injection; the agent must provide the session and token explicitly.

To retry committed writes whose responses were lost, the extension caches the originally injected tokens for its 512 most recent write requests in the current instance. This cache does not persist across host restarts. New requests must not arbitrarily reuse old idempotency keys.

Pi/OMP reports periodic heartbeat failures and retries on later ticks. Lease expiry remains the fallback if the process exits, pauses for too long, or cannot schedule its timers.

### 5.3 Agentix IM integration

taskcli does not depend on the IM bridge process. When Agentix task boards are enabled, IM can browse Tasks, and a bound session can use buttons to claim, start, or change state. Agentix validates these actions through the same Service and handles resumption or exit processing from session events.

IM does not create Plans or replace the Skill. Its existing refresh loop also consumes task events and sends waiting-user, blocked, failed, or Job-completion notifications to the corresponding session. This is not a file watcher.

## 6. Concurrency: Three Different Locks

| Mechanism | Scope and duration | Purpose |
| --- | --- | --- |
| SQLite write transaction | A short database-level write lock from `BEGIN IMMEDIATE` through commit | Make state reads, validation, and lease writes atomic to prevent duplicate claims |
| Task lease | Valid for 15 minutes after claim, renewable through heartbeats | Maintain ownership during planning and execution and reject stale session/token writes |
| Document output lock | Held during Plan publication, start's Plan validation and commit, and projection synchronization | Serialize shared document writes and prevent start from interleaving with managed Plan writes |

When two agents claim the same Task, the first to commit gets the lease. The second sees the updated state inside its transaction and receives a conflict. A primary key also ensures one lease per Task, while a unique constraint limits each executor/session pair to one Task.

The database lock is not held after commit while the agent works. Different Tasks can therefore be planned, implemented, and tested concurrently. Short database writes and document output queue behind their locks; this does not serialize all task execution.

Two request-level protections complement these locks:

- `--expect-revision` prevents decisions based on an old revision from overwriting newer state. Use it when updating previously read task information. Plan publication changes the Task revision, so refresh it before subsequent operations.
- `--idempotency-key` returns the original result when the same request is retried, without duplicate entities or events; reusing the key for a different request is rejected. A replayed result is historical and does not establish current lease ownership. Recheck context before continuing work.

### Protection boundaries

- The supported target is multiple processes sharing SQLite on one computer, not a shared database on a network filesystem.
- Task leases do not lock Git files or terminate an agent that loses its lease. Code Tasks need separate branches/worktrees and explicit coordination of shared resources.
- Direct database edits, token sharing, and direct overwrites of Plan files bypass the normal coordination boundaries. Leases are not operating-system access controls.
- Context currently selects the first active lease for a session. If future team members share a session but claim Tasks under different executors, the existing automatic context and token injection cannot be assumed to distinguish them. Use separate sessions or add explicit Task/member addressing in the Team adapter.

## 7. Interruptions, Expiry, and Resumption

Automatic resumption applies to system interruptions, not manual blocks:

1. Claim enters PLANNING; heartbeats are required throughout planning too.
2. SessionEnd marks the Task as system BLOCKED and releases its lease. A forcibly killed process may never get a chance to run the hook.
3. Without a normal exit event, later CLI/library operations or Agentix refreshes check lease expiry and mark expired Tasks as system BLOCKED. No continuously running expiry scanner is required.
4. SessionStart for the same session attempts to reclaim its previously system-blocked Tasks. Resumption fails if another executor has taken over, the Job has closed, or other claim constraints are not met.
5. Successful resumption issues a new token and returns to PLANNING. It does not automatically resume execution, even if the previous phase was EXECUTING.
6. The agent reviews context, the workspace, and existing work, completes or revises the Plan, and explicitly starts before executing. The old token cannot publish a Plan, start, or finish the Task.

A Plan that has not yet been created, or a missing Plan file, does not prevent resuming planning ownership, but it prevents start. Manual `block`, `wait`, and `release` are not automatically resumable system blocks; they require an explicit new claim.

This distinction prevents a session restart from being mistaken for confirmation that dependencies and the execution workspace are still safe.

## 8. Why Documents Need No Watcher

SQLite is authoritative for task status, dependencies, revisions, ownership, and other metadata. Board, Dashboard, and Job task sections are logically read-only views generated from those facts. TaskNotes card edits and Kanban dragging do not feed status changes back into the system.

Both output formats generate `Board.md` with an embedded TaskNotes Base. It selects Task notes in the exact project folder by project ID and archived state. Each Task has one file under `Tasks/`, whose frontmatter records status and metadata and whose body contains the Plan. Jobs link these notes directly, so their checklists and authored Plan checklists do not duplicate task cards. Link syntax remains format-specific: wikilinks for Obsidian and relative Markdown links for ordinary directories. Rendering requires TaskNotes and Bases; generating Markdown does not modify vault settings.

Obsidian with both plugins enabled is the recommended viewing environment; use `--format obsidian` when initializing against a vault. Plain Markdown mode remains available for CLI-only workflows and other editors. This recommendation changes neither SQLite ownership rules nor the requirement to route task-state changes through taskcli or Agentix.

TaskNotes controls remain editable. Dragging cards or using card menus may change the projected Markdown until the next sync. Those edits cannot obtain a lease or change SQLite task state. This is a logical read-only boundary, not a complete UI or filesystem lock, and still needs no watcher.

Plan bodies live in the Task notes, alongside frontmatter properties. Projection preserves editable Goal/Notes sections, while an explicit `job update --goal` replaces the Goal. Agents must publish Plans through `plan create/revise`, not overwrite registered files directly.

- For Obsidian, agents use the separate Obsidian Skill to author bodies with `[[wikilinks]]`. If a temporary draft is needed, use a session-specific path and publish through taskcli with the lease.
- For plain Markdown, use relative `[label](path.md)` links; the directory need not be an Obsidian vault.
- taskcli generates directories and projections deterministically. It does not start a model or automatically invoke the Obsidian Skill.

Task-state writes normally commit to the database before updating projections. A projection failure returns `projection_pending`, meaning the state change succeeded and `sync` should repair the view; do not recreate the Task. Plan publication validates and writes the file before registering metadata transactionally. The filesystem and SQLite do not share one atomic transaction, so an interruption can leave an unregistered file. The implementation checks existing content at that path instead of blindly overwriting it.

Manual board edits are never imported into SQLite and are overwritten by the next projection. Manual Plan-body edits can refresh hashes through `sync` or `plan show`, but this provides no concurrent ownership protection and is not an agent collaboration workflow.

## 9. Extending the System for Agent Teams

The system already provides stable Job/Task IDs, individual Task claims, leases, `context` queries, cursor-based events, and optional `delegated_by=team:<id>` metadata. A Team can organize members around a Job and let them claim different Tasks.

Team membership, automatic scheduling, shared-context storage and version-conflict handling, Job decomposition locks, code merging, and automatic business acceptance are not implemented. `delegated_by` records origin only; it does not authorize other Team members to use a lease.

A future layer could assign the coordinator responsibility for requirements and the task graph, key shared context by Job ID, and let members read shared facts before claiming Tasks. Versioned documents and events can collect deliverables and verification results. Concurrent shared-context writes still need a Team-level design; Task leases do not solve that problem.

## 10. Implementation and Validation References

| Mechanism | Code or tests |
| --- | --- |
| Agent working instructions | [SKILL.md](../plugins/agent-task-manager/skills/agent-task-manager/SKILL.md), [command reference](../plugins/agent-task-manager/skills/agent-task-manager/references/commands.md) |
| Four-host hooks, context injection, and tool adaptation | [runtime.mjs](../plugins/agent-task-manager/runtime.mjs), [hooks.json](../plugins/agent-task-manager/hooks/hooks.json) |
| Context and hook command entrypoints | [taskcli/main.rs](../crates/taskcli/src/main.rs) |
| Claim, start, done, and session-resumption state machine | [mutations.rs](../crates/agentix-task/src/mutations.rs) |
| Write transactions, lease expiry, and idempotent replay | [store.rs](../crates/agentix-task/src/store.rs), [schema.sql](../crates/agentix-task/src/schema.sql) |
| Plan publication, output locking, and projection | [projection.rs](../crates/agentix-task/src/projection.rs) |
| Concurrent claims, state transitions, and resumption tests | [task_system.rs](../crates/agentix-task/tests/task_system.rs), [CLI integration tests](../crates/taskcli/tests/cli.rs) |
| Actual CLI and host-adapter entrypoint integration tests | [integration.mjs](../plugins/agent-task-manager/tests/integration.mjs) |

Existing tests cover competing CLI processes, phase transitions, stale tokens, planning resumption without a Plan, Plan validation, idempotent retries, and hooks calling the actual CLI. Host-adapter tests use event and API harnesses; they do not establish that a real model always decomposes work correctly, reads the Skill, or verifies acceptance. Live host loading, trust settings, and Obsidian desktop rendering remain separate acceptance checks.
