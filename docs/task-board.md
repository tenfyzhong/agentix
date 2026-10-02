# Task board and standalone CLI

The user guide has moved to the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki/Taskix). Start with the [Wiki home](https://github.com/tenfyzhong/agentix/wiki/Home) for installation, configuration, and everyday use.

The implementation and development notes below remain in the repository.

## Validation

`make check` installs locked plugin dependencies, then runs Rust formatting, Clippy, workspace tests, and the Node built-in plugin tests. Install Node.js 24+ and npm. Direct Cargo invocations require `npm ci --ignore-scripts --prefix plugins/taskix-manager` first. Normal tests use temporary databases/directories and local mock services, not live accounts.

| Boundary | Automated coverage |
| --- | --- |
| State and ownership | Seven Task states with IN_PROGRESS split into two phases against ten commands (80 cases), no partial writes on rejection, claim-before-Plan, Plan/start ownership, missing/blank Plans, lease renewal/recovery/handoff, stale tokens, dependency changes, archival |
| Processes and storage | Eight competing CLI processes with exactly one claim winner; four concurrent Jobs; start waits for Plan writes and rechecks lease expiry; v1 phase migration preserves leases/timestamps; kill after SQLite commit but before projection, then replay without duplicate events and repair files |
| Document projection | Legacy Dashboard migration to Bases, collision preservation and retry, archive visibility and activity sorting; seven-state Mermaid graphs with cross-Job prerequisites and renamed links; TaskNotes Bases and per-task Obsidian notes, exact folder/project scope, frontmatter state projection and repair without SQLite writes, legacy Plan-path migration, editable Notes under concurrent writes, safe marker/symlink failures, and YAML-frontmatter Plan bodies |
| Host plugin | Actual Pi/OMP TypeScript entrypoints and lifecycle hooks invoke the compiled CLI; structured tool schema, plans, leases, Obsidian wikilinks, retry identity after lease release/lost responses and deletion, ordinary Claude failures and automatic Pi/OMP continuations that retain ownership, errors, aborts, identity fencing, and periodic heartbeat behavior |
| IM orchestration | Dashboard → project Jobs → Job ↔ Task navigation, attached-session scope and released work, sorted contextual menus, project/Job/task pagination, archive filtering, missing-document fallbacks, long Markdown/reasons/titles, read-only snapshots; session/revision/owner scoping, Wait/Fail reasons, cancellation, Job completion, notification paging, route isolation, retry after channel failure, durable delivery cursor after Engine reconstruction |
| Channel adapters | Actual Telegram HTTP and Feishu HTTP/WebSocket adapters exercise dashboard and detail navigation before attachment, attached `/board` and `/jobs`, and MarkdownV2/card output; task callbacks and reason messages verify SQLite state, projected Markdown, and notifications at local mock APIs |

The plugin tests use a minimal host API harness, not installed Pi/OMP loaders or model-generated tool calls. Codex/Claude hook tests execute their manifest commands with representative payloads, not live host sessions. CI runs the normal suite on Linux/macOS and task core/CLI/plugin checks on Windows; Unix-only symlink cases are excluded on Windows.

For actual Obsidian rendering, enable the ignored desktop tests explicitly. Open a test vault with TaskNotes and Bases enabled, bring its window to the foreground, enable the Obsidian CLI, and ensure the chosen parent directory already exists:

```sh
TASKIX_OBSIDIAN_VAULT="Test vault" TASKIX_OBSIDIAN_PARENT="Tests" \
  cargo test -p taskix --test obsidian_smoke -- --ignored --nocapture
```

`OBSIDIAN_BIN` can select a specific CLI executable. For each format, the test creates an isolated `taskix-smoke-*` directory under that parent (default `00-Inbox/agent`) and a temporary tab, then restores the previous tab and deletes only its own generated files. It checks Dashboard columns, dates, archive/unarchive filtering and link targets through native navigation, plus separate Job/Task Kanban columns and cards, task note recognition, note links, and the Mermaid state diagrams. The status bridge scenario loads the bundled plugin under a temporary ID against an isolated database, edits real frontmatter, and verifies rollback notifications and successful CLI writes. The visibility prerequisite prevents hidden-window rendering from being mistaken for an empty board. It temporarily extends TaskNotes status definitions in memory and loads a temporary Taskix Sync instance through Obsidian's plugin loader; cleanup unloads it and restores the previous status definitions. It does not change persistent community-plugin settings. A force-killed test process can leave its temporary directory/tab behind; do not run it concurrently with manual edits in that directory.

The [integration coverage map](integration-coverage.md) links each behavior to its executable tests. These tests do not establish complete branch coverage or live-system acceptance. Real IM credentials/permissions, host installer and loader compatibility, model-directed tool selection, desktop themes/plugins, and multi-machine/network-filesystem behavior require separate checks. The supported concurrency target remains multiple local processes on one computer.
