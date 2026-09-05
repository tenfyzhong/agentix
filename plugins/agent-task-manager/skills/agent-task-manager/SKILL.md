---
name: agent-task-manager
description: Track multi-step or cross-session work with taskcli jobs, task claims, plans, and read-only Obsidian or Markdown boards. Use when work benefits from durable progress or several agents coordinate tasks; avoid creating a job for every short question.
---

# Agent Task Manager

Use `taskcli --json` or the host's `taskcli` tool. The command reference is [references/commands.md](references/commands.md). Read it when creating work or changing state.

One Project represents a long-lived repository; worktrees of that repository share the Project. One Job is an independently acceptable requirement. Tasks are executable pieces with one agent/session lease each. New requirements after Job completion get new Jobs. Separate Tasks and Jobs may run concurrently.

## Begin or resume

1. Read `taskcli context --session <host-session-id> --json`. Use the actual host session ID from SessionStart or the extension; do not invent one. Preserve a caller-provided `job_id` and Task assignment. A Team's shared context is external and keyed by `job_id`.
2. Discover or register the Git Project with `project register`; non-Git work requires explicit `--project`. Inspect existing active Jobs before creating another for the same request.
3. Put the goal and acceptance checks in the Job. Split work into independently executable Tasks and add dependencies before any affected Task starts. Do not represent TDD as a test Task that the production Task can bypass: each behavioral Task includes its own failing test, implementation, and passing validation.
4. Create a Plan just before beginning a Task. It must specify intended changes, validation, and relevant constraints. Claim only after dependencies are DONE. Keep the returned lease token and revision.

## Execute and hand off

- Claim with `--executor`, `--session`, and optional `--delegated-by team:<id>`. A Team member owns the lease directly. A claim conflict means another executor owns the Task; inspect status before selecting other work.
- Supply the current session and lease token for writes to a leased Task, including Plan revision. Use `--expect-revision` when updating previously read state. Keep the same idempotency key and arguments when retrying the same operation; never reuse it for changed intent.
- Heartbeat leases at least once per minute during long work. Pi/OMP extensions do this while the host is running. Codex/Claude hooks renew at tool boundaries; a single operation or inactive gap longer than 15 minutes can expire the lease. After expiry, stop task writes, inspect state, and reacquire only if available. A completion must not be reported after a rejected stale write.
- Use `done` only after acceptance checks pass. Use `block`, `wait`, or `fail` with a concrete reason; use `release --reason` for handoff. `retry` and `reopen` are explicit operations. Terminal, blocked, and waiting transitions release the lease.
- Create a new Plan version with `plan revise`. After directly editing a current Plan or Notes, run `sync` to refresh hashes. A projection warning means the database write succeeded; run `sync` to repair without creating duplicate work.
- Code Tasks use separate branches/worktrees according to repository instructions. Task leases coordinate metadata; they do not isolate source files automatically.

## Document editing

Read `context.documents.format` before writing documents. Generated Dashboard, Board, and Job task sections are logically read-only. Change titles, status, dependencies, ordering, or ownership only through taskcli. Do not use Kanban dragging, Tasks checkboxes, or a file watcher.

When format is `obsidian`, use the installed Obsidian skill for direct Plan/Goal/Notes creation or editing, preserving configured paths and internal `[[wikilinks]]`. If the skill is unavailable, explain the missing dependency; continue task metadata operations, but do not silently substitute a different Obsidian editing workflow. When format is `markdown`, use standard relative `[label](path.md)` links. `taskcli` generates managed sections deterministically in either format.

Keep the editable section markers intact. Never write Team Context into generated sections or treat a document's manual status edits as database facts. Archive completed or cancelled Jobs only when requested or when the user's established workflow calls for it.
