---
name: agent-task-manager
description: Track multi-step or cross-session work with taskcli jobs, task claims, plans, and TaskNotes-compatible Obsidian or Markdown views. Use when work benefits from durable progress or several agents coordinate tasks; avoid creating a job for every short question.
---

# Agent Task Manager

Use `taskcli --json` or the host's `taskcli` tool. The command reference is [references/commands.md](references/commands.md). Read it when creating work or changing state.

One Project represents a long-lived repository; worktrees of that repository share the Project. One Job is an independently acceptable requirement. Tasks are executable pieces with one agent/session lease each. New requirements after Job completion get new Jobs. Separate Tasks and Jobs may run concurrently.

## Task language

Use `AGENT_TASK_LANG` from the host environment to choose the language for task decomposition and newly authored Job/Task titles, concise names, goals, Notes, and Plan bodies. The plugin also exposes this setting as `task_language` in its injected context. Unset or blank means English (`en`); for example, `zh-CN` selects Chinese and `ja` selects Japanese. Honor an explicit language requested for the current work. Preserve existing authored content unless translation is requested.

This is a skill setting. Do not write language settings into taskcli configuration or translate its generated sections. taskcli stores supplied text as-is and uses fixed English template labels.

## Begin or resume

1. Read `taskcli context --session <host-session-id> --json`. Use the actual host session ID from SessionStart or the extension; do not invent one. Preserve a caller-provided `job_id` and Task assignment. A Team's shared context is external and keyed by `job_id`.
2. Discover or register the Git Project with `project register`; non-Git work requires explicit `--project`. Inspect existing active Jobs before creating another for the same request.
3. When creating Jobs or Tasks through the shell, supply `--executor agent:HOST --session HOST_SESSION` using the actual host (`codex`, `claude`, `pi`, or `omp`) and session ID so their frontmatter records provenance. Pi/OMP’s structured tool supplies these automatically. Decompose the Job into a directed acyclic graph (DAG) using the criteria below: nodes are Tasks and an edge A → B means B requires A to finish. Identify every known node and prerequisite edge before configuring task dependencies. Give each Job and Task a concise descriptive `--name` in the configured task language. Put the overall goal and acceptance checks in the Job, and each Task's bounded deliverable and completion condition in its `--title`; defer the execution approach to its Plan.
4. Create or reuse a Task for each DAG node. Use `task add` for new nodes and retain a node-to-Task-ID mapping. Each add creates its Task file with managed frontmatter, without publishing a Plan. Do not create duplicate Tasks for existing nodes.
5. Once the nodes have Task IDs, configure every DAG edge A → B with `task depend B_ID A_ID`. For multiple prerequisites, add one dependency per incoming edge. Verify each Task with `task show` and compare its generated `dependencies` frontmatter against the DAG; check `task list --ready` for the expected entry points. Repair projection warnings with `sync`, and resolve missing or rejected edges before implementation. Do not create Task files or edit managed frontmatter by hand.
6. When ready to work on a Task, prefer `task list --ready`, inspect its prerequisites with `task show`, and claim it before drafting its Plan. A successful claim reserves planning ownership (`IN_PROGRESS` / `PLANNING`); keep the lease token. If claim fails, do not write that Task's Plan or start its work. Leave other Task notes without published Plans until their execution approach needs to be prepared.
7. While holding the lease, publish the Plan into that same Task note through `plan create/revise`. Use the actual prerequisite outputs to describe the approach and validation, with freely chosen content and structure. Call `task start` with the same lease only after reviewing the Plan and confirming all prerequisites are DONE. Proceed with implementation only when start succeeds (`EXECUTING`); refresh the Task revision after Plan writes.

## Decomposition and prerequisites

- Split by verifiable deliverables, interfaces, or independently investigable questions. Each Task should fit one ownership interval and have clear inputs, scope, and a completion condition. Keep a small cohesive change in one Task; split work that has distinct outcomes or can proceed independently. Do not create Tasks for every file, command, or routine step.
- Model real output dependencies: if B consumes an interface, decision, migration, or artifact produced by A, B depends on A. Independent branches can converge on a downstream integration Task. Avoid artificial sequential chains, and do not omit necessary edges to suggest more concurrency. Dependencies must be acyclic and within one Project; they may cross Jobs.
- Keep each behavioral Task's failing test, minimum implementation, and passing verification together. A downstream integration or acceptance Task can validate the combined result, but cannot replace those tests.
- Review that the graph covers the Job's acceptance criteria before implementation. If research is needed to discover the remaining scope, register that research and the known downstream deliverables first, then refine them before they start. Use `task depend/undepend` for changes; dependencies cannot change once that Task has started execution.
- Task frontmatter contains `dependencies: []` when there are no prerequisites, or a list of prerequisite Task IDs. This field is generated from SQLite, including during Plan publication and sync. Read current status from taskcli: editing the file or describing dependencies only in prose does not establish or satisfy execution dependencies.

## Execute and hand off

### Project Inbox

Humans submit requirements in the Project's `Inbox.md`, or through IM `/inbox <content>`. Each top-level checkbox is one submission. Inbox states are TODO, IN_PROGRESS, DONE, and CANCELLED; the associated Job retains its normal lifecycle. `context` provides `inbox_path`, an owned `inbox`, and `inbox_cancellations`.

- Before ending tracked work, use `inbox claim-next --project PROJECT_ID` with your executor/session. Only take new submissions after every existing Job in that Project is completed or cancelled. BLOCKED and WAITING_USER Tasks still keep their Job active. The CLI atomically reserves one entry and creates its Job; use the returned Job instead of calling `job create` again.
- If a hook already claimed an entry, inspect `context` and adopt its Job. Register missing Task nodes and dependencies, then use the normal claim → Plan → start → verify → done workflow. On recovery, inspect existing Tasks and outputs before decomposing; never duplicate the existing Job or Task graph.
- Inbox ownership has its own lease, renewed by the same session heartbeat. `inbox release ENTRY_ID` requires the Inbox token, not a Task token. Use the full entry ID with the Pi/OMP structured tool. Interruption, release, or expiry returns unfinished entries to TODO so another agent can resume the same Job. An IN_PROGRESS entry cannot be taken by another agent.
- Completion of the associated Job marks the entry DONE. Check again for the next entry until the queue is empty. Stop hooks are a fallback for omitted checks; they do not provide a persistent idle watcher or override the host's current mode.
- A human can cancel with `- [-]` or withdraw by deleting an unfinished entry. Synchronization cancels its associated unfinished work and revokes leases. On cancellation facts or stale-token rejection, stop that Job at the next safe boundary, preserve completed results, and do not automatically roll back code, recreate the Job, or retry its writes. Running external commands may still need to finish.
- Do not edit entry IDs or generated state/Job links. Manually checking a box does not complete work, and unchecking a cancelled entry does not restart it. Missing, unreadable, or malformed Inbox documents require repair; do not interpret them as mass cancellation.

### Task ownership

- Claim with `--executor agent:HOST:MEMBER` (using the actual host and a member/session suffix when needed), `--session`, and optional `--delegated-by team:<id>`. A Team member owns the lease directly. A claim conflict means another executor owns the Task; inspect status before selecting other work.
- Both `plan create` and `plan revise` require the current session and lease token; never submit a Plan for an unclaimed Task. Supply them for `start` and other writes to a leased Task too. Use `--expect-revision` when updating previously read state. Keep the same idempotency key and arguments when retrying the same operation; never reuse it for changed intent.
- Heartbeat leases at least once per minute during planning as well as execution. Pi/OMP extensions do this while the host is running. Codex/Claude hooks renew at tool boundaries; a single operation or inactive gap longer than 15 minutes can expire the lease. After expiry, stop task writes, inspect state, and reacquire only if available. Session recovery gets a new token and returns to PLANNING, even when an existing Plan is missing; repair/review the Plan and explicitly start before continuing execution. A completion must not be reported after a rejected stale write.
- Use `done` only in EXECUTING, after acceptance checks pass. Use `block`, `wait`, or `fail` with a concrete reason; use `release --reason` for handoff. `retry` and `reopen` are explicit operations. Terminal, blocked, and waiting transitions release the lease and clear the phase.
- Publish Plan bodies through `plan create` / `plan revise`; do not directly overwrite registered Plan files. A Plan revision updates the same Task note in `Tasks/`, advances the Task revision, and requires ownership; do not create a `Plans/` directory, version directories, or intermediate Plan files. After editing Notes, run `sync`. A projection warning means the database write succeeded; run `sync` to repair without creating duplicate work.
- Code Tasks use separate branches/worktrees according to repository instructions. Task leases coordinate metadata; they do not isolate source files automatically.

## Document editing

Read `context.documents.format` before writing documents. Each Task has one TaskNotes-compatible note in `Tasks/`, created with task metadata before a Plan is published. Its frontmatter records status, IDs, prerequisite Task IDs in `dependencies`, local dates, and revision; its body holds the Plan once prepared for that Task. Job and Task frontmatter also records managed `agent` and `session_id`: the Job creator, and the Task creator until replaced by its latest claimant. These survive completion; unknown values remain `null`. Job task sections directly link Task notes. Board embeds a Bases view over task frontmatter. Generated Dashboard, Board, and Job task sections are logically read-only. Change titles, status, dependencies, ordering, or ownership only through taskcli. Do not use TaskNotes card edits, Kanban dragging, or a file watcher to update managed state.

When format is `obsidian`, use the installed Obsidian skill to author Plan bodies and edit Goal/Notes, preserving configured paths and internal `[[wikilinks]]`. Draft a Plan only after claim, in a session-specific temporary file if needed; publish it through taskcli with the lease so ownership is checked before its registered file is created. If the skill is unavailable, explain the missing dependency; continue task metadata operations, but do not silently substitute a different Obsidian editing workflow. When format is `markdown`, use standard relative `[label](path.md)` links in authored prose; generated Job-to-Task references use `[task filename](path.md)`. Preserve YAML frontmatter and type tags on Markdown documents; the generated `Dashboard.base` uses native Base YAML instead of Markdown frontmatter. `taskcli` generates managed sections deterministically in either format.

Keep the editable section markers intact. Never write Team Context into generated sections or treat a document's manual status edits as database facts. Archive completed or cancelled Jobs only when requested or when the user's established workflow calls for it.
