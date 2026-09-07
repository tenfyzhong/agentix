# Using TaskNotes in Obsidian

Each taskcli Task is a Markdown note in its project's `Tasks/` directory. TaskNotes reads its frontmatter and displays it in the project Kanban board. The agent freely organizes the note body to suit the task; no section template is required.

## Enable the views

After configuring taskcli with `init --format obsidian`, close the vault in Obsidian and run `taskcli obsidian setup`. This installs TaskNotes and the embedded Taskcli Sync desktop plugin, enables both and Bases in the vault configuration, and merges the settings described below. Reopen Obsidian and disable Restricted mode if needed. Existing compatible installations are retained; `--plugin-dir /path/to/release` supports offline installation. Changed files are backed up under `.obsidian/taskcli-backups/`. See [automatic setup](https://github.com/tenfyzhong/agentix/blob/main/docs/task-board.md#obsidian-plugin-setup) for details.

**TaskNotes** (`tasknotes`) renders the generated notes and boards. **Taskcli Sync** (`taskcli-sync`) submits status edits to taskcli. Also enable **Bases**, which is built into Obsidian. This integration was checked with TaskNotes 4.12.5.

In **Settings → TaskNotes → General**, select tag-based task identification and set the task tag to `task`. This identifies the `tags` property of a task note; Job links and checkboxes inside a plan do not create extra task cards. Task notes carry both `task` for TaskNotes identification and `agent/task` for Agentix board filtering, so ordinary `task` notes can coexist in the same vault. Run `taskcli sync` with the updated CLI to add the tag to existing generated notes before changing this vault-wide setting. Custom tags are preserved.

Keep the default field mappings for `title`, `status`, and `projects`. Map TaskNotes `dateCreated` to `created_at`, `dateModified` to `updated_at`, and `completedDate` to `completed_at`. The bundled preset and `taskcli obsidian setup` apply these mappings; the internal TaskNotes names must not become additional note properties. Keep the archive tag mapping as `archived`. Set **Open task after creation** to **None** to keep newly created notes from opening automatically.

See [TaskNotes core concepts](https://tasknotes.dev/obsidian/core-concepts/) for the note model and field mapping.

## Configure Task and Job statuses

In **Settings → TaskNotes → Task Properties**, add the following status values. Values must match the generated frontmatter exactly. Labels can be customized; the supplied labels are English.

| Value | Label | Color | Completed |
| --- | --- | --- | --- |
| TODO | Todo | `#cbd5e1` | No |
| IN_PROGRESS | In Progress | `#bfdbfe` | No |
| BLOCKED | Blocked | `#fed7aa` | No |
| WAITING_USER | Waiting User | `#ddd6fe` | No |
| DONE | Done | `#bbf7d0` | Yes |
| FAILED | Failed | `#fecaca` | No |
| CANCELLED | Cancelled | `#e2d7e7` | No |
| ACTIVE | Active | `#bfdbfe` | No |
| PENDING_REVIEW | Pending Review | `#fed7aa` | No |
| COMPLETED | Completed | `#bbf7d0` | Yes |

These light status colors are shared with Mermaid node backgrounds. Mermaid labels use dark `#1f2937` text for contrast. Apply these color values to existing TaskNotes settings when updating from an older palette.

Disable automatic archival for these statuses. Failed and Cancelled are terminal taskcli states, but are not successful completion. They have separate board columns. Replace unused default statuses with these values and set the default status to `TODO`. Preserve definitions that other notes use. Job-only statuses are excluded from the Task status cycle. Each board pins its own status columns and uses `hideEmptyColumns: true`, keeping those columns visible while hiding unrelated empty columns.

The reusable [tasknotes-settings.json](tasknotes-settings.json) contains this settings subset. Merge it with existing settings; do not replace the entire TaskNotes configuration. taskcli does not install plugins or change vault-wide settings during sync.

## Project structure

```text
11-Agents/
  Dashboard.base
  Recent Jobs.base
  Projects/<project>/
    Board.md
    Jobs/
      YYMMDD-seq-<job-name>.md
      Archived/YYMMDD-seq-<job-name>.md
    Tasks/
      YYMMDD-seq-<task-name>.md
```

- **Dashboard.base** is a compact native table of active projects: Name (click to open Board), Status, and Updated (recent project activity). It uses read-only formula columns and hides archived projects. Its **Pending review** Kanban view lists all unarchived PENDING_REVIEW Jobs, oldest `pending_review_at` first. Sync safely replaces the old generated Dashboard.md; Markdown output uses a portable table instead.
- **Recent Jobs.base** opens a cross-project board with ACTIVE, PENDING_REVIEW, COMPLETED, and CANCELLED columns. Each column shows up to ten unarchived Jobs, newest `updated_at` first. Taskcli Sync supplies the limited TaskNotes Kanban view; four native status tables also limit results to ten each. Cards show the project Board link, local update time, and pending-review time when present. Sync safely migrates the registered `Pending Review.base` to the new filename.
- Card single-click opens the original Job or Task note directly. `taskcli obsidian setup` persists TaskNotes' `singleClickAction: openNote` preference.
- **Board.md** records repository identity, paths, project state, and sync status, and embeds two Bases views of type `tasknotesKanban`, grouped by status: Job board above Task board. It is the project note, with the Project ID and both `agent/project` and `agent/board` tags. Dashboard and task project links point here; there is no separate Project link on Board. Sync removes the old generated `meta.md` after publishing Board.
- **Job → Tasks** directly links the task notes, using their filenames as labels.
- **Tasks/** contains one note for every Task, including tasks without a published plan.

Open Board in Reading view or Live Preview. Each Base filters the exact project's `Jobs/` or `Tasks/` folder, project ID, corresponding `agent/job` or `agent/task` tag, and `archived != true`. Jobs expose `title`, `created_at`, `updated_at`, and `completed_at` through the configured TaskNotes mappings without carrying the `task` tag. Both views sort by `updated_at` ascending, with filename breaking ties, and use 300px columns. Completed tasks remain visible until their Job or Project is archived. No generated checkbox lists are used as the view's data source.

## Task properties and plan body

A task note has this shape:

```yaml
---
id: task_example
task_id: task_example
plan_id: plan_example
project_id: prj_example
job_id: job_example
title: Implement login
status: IN_PROGRESS
phase: EXECUTING
dependencies:
  - task_prerequisite
revision: 4
sequence: 1
tags:
  - agent/task
  - task
archived: false
projects:
  - "[[11-Agents/Projects/example/Board]]"
job: "[[11-Agents/Projects/example/Jobs/260905-0001-Login]]"
created_at: 2026-09-05T08:00:00+08:00
updated_at: 2026-09-05T08:01:00+08:00
started_at: 2026-09-05T08:01:00+08:00
completed_at: null
---
```

The note's ID identifies the Task. `plan_id` identifies its published plan, and is null until the first plan is published. `revision` is the only revision field in the document and advances with taskcli changes, including Plan publication. The legacy `version` property is removed from Tasks. All generated documents use one property per lifecycle timestamp: `created_at`, `updated_at`, `started_at`, `completed_at`, `pending_review_at`, `cancelled_at`, and `archived_at`, where applicable. Sync and status repair remove `dateCreated`, `dateModified`, and `completedDate`; existing Inbox `created`/`updated` properties migrate to `created_at`/`updated_at`. Canonical values take precedence over duplicate aliases. Lifecycle timestamps use the computer’s local time zone, with an explicit UTC offset appropriate to each instant. TaskNotes reads and writes the canonical properties through its field mappings. Job lifecycle fields, including `created_at`, `completed_at`, and `pending_review_at`, also use local time.

`dependencies` lists prerequisite Task IDs, or is `[]` when there are none. Create the known Tasks and register their dependencies with `task depend TASK PREREQUISITE` before implementation; taskcli creates their files and manages the frontmatter. Existing notes gain this property on sync. Dependency edits must go through taskcli, and `task start` requires every prerequisite to be DONE in the database.

Job task sections include a generated Mermaid graph with arrows from prerequisites to dependent Tasks. Independent Tasks remain visible, and direct prerequisites from other Jobs include their Job name. Each node shows the Task name and exact status, using the seven colors in the configuration above. Click its label to open the Task note through an Obsidian internal link, or hover to preview it. The graph refreshes with taskcli status changes or sync, and task renames refresh its links. These nodes are read-only status displays, not editable TaskNotes widgets; custom vault colors do not change their palette. Ordinary task links remain below the graph for viewers that disable Mermaid links.

When taking up a Task, claim it and publish its Plan into the same note. Other Tasks can remain as metadata notes until their Plans are needed. Organize the body freely around the needs of the task; headings and content are chosen by the agent. Preserve research notes, examples, and checklists that help execute the task. `AGENT_TASK_LANG` controls the language of agent-authored names and prose; it does not translate plugin status values.

Agents continue using `claim → plan create/revise → start → done`. The `plan` commands publish the body of the same Task note; they do not create another directory or a version file. Merely creating a Task note does not satisfy the requirement to publish a plan before starting execution.

## Migrate existing projects

Back up the task database and generated documents, update all taskcli writers, and run:

```sh
taskcli sync
taskcli doctor --json
```

Database schema 7 migrates the registered Plan paths from `Plans/` to `Tasks/`. Sync preserves the latest plan body and authored properties, updates Job links and Board, and removes old managed files after publishing their replacements. The former `Tasks.md` list and links to it are removed; status is viewed in Board. Unrelated files are not deleted. A destination conflict leaves the original content available; resolve it and run sync again.

The old `kanban-plugin` and `kanban_plugin` properties are removed. Close and reopen any old Board tab in Markdown Reading view so the embedded Base renders. Existing Project and Job hierarchy, task identities, sequence prefixes, and internal Plan publication counters are preserved.

Job archival keeps task notes in `Tasks/`, sets `archived: true`, and adds TaskNotes' `archived` tag. Unarchiving clears both. Job deletion removes related task notes, including notes without a plan; Project deletion removes the entire generated project directory.

## State changes and styling

With Taskcli Sync enabled, saved frontmatter `status` edits and TaskNotes status dragging call taskcli. Configure the executable and config file in **Settings → Taskcli Sync**, then click **Connect**. The button shows **Checking...** and is disabled during the check. A successful check reports the monitored directory; a failure displays its reason and allows another attempt. The command palette connection check provides the same feedback, while successful automatic startup stays quiet. Initial setup fills absolute paths and preserves user settings on later runs. The plugin is desktop-only and requires Obsidian 1.10.1 or later.

### Status edits

File events are monitored only inside the configured `documents.directory`, including its subdirectories. Creating, editing, deleting, or renaming notes elsewhere in the vault does not trigger synchronization, even if they contain copied taskcli properties or Inbox markers. Moving a file into or out of the monitored directory refreshes registered paths. Setting the directory to `.` monitors the whole vault. If the initial connection fails, automatic event handling stays paused until a successful connection check.

| Entity | Requested status | CLI operation |
| --- | --- | --- |
| Task | BLOCKED / WAITING_USER / FAILED / CANCELLED | block / wait / fail / cancel, subject to normal state and lease guards |
| Task | TODO from FAILED | retry |
| Task | TODO from DONE or CANCELLED | reopen |
| Task | IN_PROGRESS or DONE | Rejected; use the owning agent's claim, Plan, start/done workflow |
| Job | ACTIVE → PENDING_REVIEW | submit, when all non-cancelled Tasks are DONE and at least one exists |
| Job | PENDING_REVIEW → COMPLETED | approve after verification |
| Job | PENDING_REVIEW → ACTIVE | reject; Task states are preserved |
| Job | CANCELLED | cancel from an eligible unfinished Job |

Commands requiring a reason receive `Status changed in Obsidian: OLD -> NEW`. All other transitions are rejected. The plugin never claims Tasks or reads lease tokens. Task edits against an active agent lease fail safely.

### Inbox checkbox edits

Taskcli Sync also monitors registered top-level items in `Projects/<project>/Inbox.md`, querying each entry by the ID in its marker. Run `taskcli inbox sync` to register newly authored submissions. Nested checklists, fenced examples and copied Inbox files are ignored. Keep the generated entry ID, state/revision receipt and region markers intact; `taskcli sync` upgrades older receipts.

| Checkbox | Inbox / CLI status | Effect |
| --- | --- | --- |
| `- [ ]` | TODO | Queue the item. Reopens a terminal Job to ACTIVE or rejects a pending review while preserving Tasks. Active Inbox/Task leases must be released first. |
| `- [/]` | ACTIVE | In progress, matching an ACTIVE Job. Creates its Job if absent, reopens terminal work or rejects pending verification; does not claim an agent lease. |
| `- [r]` | PENDING_REVIEW | Submit its ACTIVE Job when all non-cancelled Tasks are DONE and at least one exists. |
| `- [x]` | COMPLETED | Approve a PENDING_REVIEW Job. An unlinked TODO item can complete directly. |
| `- [-]` | CANCELLED | Cancel unfinished work and revoke its leases. Reopen completed items before cancelling. |

Task completion automatically moves the Inbox item to `[r]` with its Job. Verification rejection returns both to ACTIVE (`[/]`); approval produces COMPLETED (`[x]`). A human can activate an item without impersonating an agent. An unleased ACTIVE entry is eligible for an explicit `inbox claim-next`; lease release/expiry returns it to TODO for recovery of the same Job. Cancelled entries must be reopened before completion. Deleted entries cannot be revived, and archived Projects/Jobs must be unarchived first. Invalid edits restore the checkbox and report the CLI error. Deletion retains the existing withdrawal behavior.

Schema 10 migrates old Inbox `IN_PROGRESS` / `DONE` values to `ACTIVE` / `COMPLETED`, recovering PENDING_REVIEW from the linked Job. The CLI still accepts the old names as aliases. Upgrade all database writers together; `sync` updates old receipts and checkbox symbols. Task states remain unchanged.

Changes debounce for 300 ms per note or Inbox entry and execute one at a time with expected revision and a unique idempotency key. Only registered paths with matching identities are eligible; copied notes are ignored. A stale revision refreshes the authoritative state. On failure, status and managed dates are restored without replacing authored bodies or custom properties, and a Notice explains the error. Inbox rollback updates only the matching checkbox and receipt, preserving other entries, links and details. Newer queued edits are protected from older rollback and projection echoes, including multiple entries in one file.

A 30-second process timeout is followed by a fresh query of the affected ID because the database may already have committed. A `projection_pending` response means success: the plugin retries `sync --pending` once and reports any remaining document failure without undoing the acknowledged state. If the CLI cannot be reached, a recently confirmed state is restored when available, with an explicit uncertainty notice. If there is no confirmed state, the plugin leaves the file unchanged and reports the failure. Open the note again or reconnect to reconcile it.

The plugin starts with `taskcli obsidian connection --json`, which reads only configuration. Opening or editing a note queries `taskcli obsidian show ID --json` for its authoritative path, status, revision, and managed display properties without exposing leases or authored content. Startup reconciles only open notes; other notes are reconciled when opened, without replaying offline drift as commands. No full-vault scan or full snapshot is needed. The plugin retains at most 128 recently confirmed records for echo suppression and recovery, plus pending edits. The existing full `obsidian snapshot` command remains available for diagnostics.

CLI writes query the affected records and commit a durable document queue with the state change. Only affected notes, Job summaries/dependency graphs, Project metadata and Inbox entries are synchronized. Job renaming and archival propagate to the associated Task notes. Unchanged content is not replaced, and unrelated Project Boards keep their own sync sequence. Pending publications are processed in batches of 128 and remain recoverable after failure or restart. Automatic recovery uses `sync --pending`; plain `sync` is the full repair/rebuild path. Schema 12 upgrades the per-document registry and schedules one full rebuild.

Update the CLI and plugin together: the plugin requires the `connection` and `show` subcommands and `sync --pending`. The configured vault root must match the open vault. Commands use a subprocess argument array, without a shell. Plugin unload clears cached and queued state and terminates its subprocesses.

TaskNotes supplies status colors from its settings.

If a board is empty, bring the vault window to the foreground (hidden windows can defer view rendering), then confirm TaskNotes and Bases are loaded, the note is in Reading view or Live Preview, the task tag is `task`, and the task's folder and `project_id` match the Base filters. For unknown-status indicators, check exact status values. For missing completed work, check `archived` and run `taskcli doctor --json`.

## Backup and database recovery

TaskNotes can display existing task notes without taskcli's database. This does not make the vault a complete SQLite backup. Notes contain current task properties and authored content, but omit leases, audit events, idempotency records, and internal synchronization state. Some task fields are only displayed in Job prose or are not exported at all.

There is no command to import the vault or rebuild SQLite from it. `taskcli sync` writes database state into documents; it does not restore a missing database. Reconstructing current work from notes would require a separate importer and validation, and could not recover all original history or coordination records.

Keep a matched backup of the SQLite database, the document tree, and taskcli configuration. Include Obsidian's TaskNotes settings for the same display on another device. SQLite alone does not retain all published plan bodies or editable Notes. See [data coverage and recovery](https://github.com/tenfyzhong/agentix/blob/main/docs/task-board.md#data-coverage-and-recovery) for the field coverage and backup procedure.

Job notes include the stored original **Prompt** and a generated **Conversation** of user prompts and agent text replies. Host hooks and Agentix omit injected AGENTS.md/environment context, tool calls, tool results, and reasoning. Agent replies appear together in one Markdown blockquote, without per-message timestamps. Sync also filters legacy injected context from existing notes. These sections are regenerated from SQLite; see [conversation capture](https://github.com/tenfyzhong/agentix/blob/main/docs/task-board.md#job-conversation-records).

Inbox items render consecutively without an extra blank line. Telegram message edits and Feishu source refreshes (every 30 seconds) update the same registered Inbox item and document while preserving its state and Job link.
