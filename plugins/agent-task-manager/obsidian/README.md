# Using TaskNotes in Obsidian

Each taskcli Task is a Markdown note in its project's `Tasks/` directory. TaskNotes reads its frontmatter and displays it in the project Kanban board. The agent freely organizes the note body to suit the task; no section template is required.

## Enable the views

**TaskNotes** (`tasknotes`) is the only community plugin required for the generated task notes and boards. Also enable **Bases**, which is built into Obsidian. This integration was checked with TaskNotes 4.12.5.

In **Settings → TaskNotes → General**, select tag-based task identification and set the task tag to `agent/task`. This identifies the `tags` property of a task note; Job links and checkboxes inside a plan do not create extra task cards. If other notes use another identifying tag, migrate them deliberately before changing this vault-wide setting.

Keep the default field mappings for `title`, `status`, `projects`, `dateCreated`, `dateModified`, and `completedDate`. Keep the archive tag mapping as `archived`. Set **Open task after creation** to **None** to keep newly created notes from opening automatically.

See [TaskNotes core concepts](https://tasknotes.dev/obsidian/core-concepts/) for the note model and field mapping.

## Configure the seven statuses

In **Settings → TaskNotes → Task Properties**, add the following status values. Values must match the generated frontmatter exactly. Labels can be customized; the supplied labels are English.

| Value | Label | Color | Completed |
| --- | --- | --- | --- |
| TODO | Todo | `#526274` | No |
| IN_PROGRESS | In Progress | `#165dcc` | No |
| BLOCKED | Blocked | `#9a5200` | No |
| WAITING_USER | Waiting User | `#7836ad` | No |
| DONE | Done | `#16703d` | Yes |
| FAILED | Failed | `#b42332` | No |
| CANCELLED | Cancelled | `#6b6273` | No |

Disable automatic archival for these statuses. Failed and Cancelled are terminal taskcli states, but are not successful completion. They have separate board columns. Replace unused default statuses with these seven and set the default status to `TODO`. Preserve definitions that other notes actually use; TaskNotes will show those additional status columns too. The project board explicitly orders taskcli values and keeps empty columns visible.

The reusable [tasknotes-settings.json](tasknotes-settings.json) contains this settings subset. Merge it with existing settings; do not replace the entire TaskNotes configuration. taskcli does not install plugins or change vault-wide settings during sync.

## Project structure

```text
11-Agents/
  Dashboard.md
  Projects/<project>/
    meta.md
    Board.md
    Jobs/
      YYMMDD-seq-<job-name>.md
      Archived/YYMMDD-seq-<job-name>.md
    Tasks/
      YYMMDD-seq-<task-name>.md
```

- **Dashboard** lists projects and view links, without Job details.
- **meta.md** records repository identity, paths, project state, and sync status.
- **Board.md** embeds a Bases view of type `tasknotesKanban`, grouped by status.
- **Job → Tasks** directly links the task notes, using their filenames as labels.
- **Tasks/** contains one note for every Task, including tasks without a published plan.

Open Board in Reading view or Live Preview. The generated Bases filters select the exact project's `Tasks/` folder, its project ID, the `agent/task` tag, and `archived != true`. Completed tasks remain visible until their Job or Project is archived. No generated checkbox lists are used as the view's data source.

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
revision: 4
sequence: 1
tags:
  - agent/task
archived: false
projects:
  - "[[11-Agents/Projects/example/meta]]"
job: "[[11-Agents/Projects/example/Jobs/260905-0001-Login]]"
created_at: 2026-09-05T08:00:00+08:00
updated_at: 2026-09-05T08:01:00+08:00
started_at: 2026-09-05T08:01:00+08:00
completed_at: null
dateCreated: 2026-09-05T08:00:00+08:00
dateModified: 2026-09-05T08:01:00+08:00
completedDate: null
---
```

The note's ID identifies the Task. `plan_id` identifies its published plan, and is null until the first plan is published. `revision` is the only revision field in the document and advances with taskcli changes, including Plan publication. The legacy `version` property is removed during sync. Lifecycle timestamps use the computer’s local time zone, with an explicit UTC offset appropriate to each instant. TaskNotes uses the corresponding camelCase date properties.

Organize the body freely around the needs of the task; headings and content are chosen by the agent. Preserve research notes, examples, and checklists that help execute the task. `AGENT_TASK_LANG` controls the language of agent-authored names and prose; it does not translate plugin status values.

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

TaskNotes can edit note properties and drag cards between columns. Those actions do not acquire a taskcli lease or change its database. Use taskcli or an agent for managed status changes; sync restores managed frontmatter. Authored body content and custom properties are preserved. This integration does not lock TaskNotes controls.

TaskNotes supplies status colors from its settings.

If a board is empty, confirm TaskNotes and Bases are loaded, the note is in Reading view or Live Preview, the task tag is `agent/task`, and the task's folder and `project_id` match the Base filters. For unknown-status indicators, check exact status values. For missing completed work, check `archived` and run `taskcli doctor --json`.

## Backup and database recovery

TaskNotes can display existing task notes without taskcli's database. This does not make the vault a complete SQLite backup. Notes contain current task properties and authored content, but omit leases, audit events, idempotency records, and internal synchronization state. Some task fields are only displayed in Job prose or are not exported at all.

There is no command to import the vault or rebuild SQLite from it. `taskcli sync` writes database state into documents; it does not restore a missing database. Reconstructing current work from notes would require a separate importer and validation, and could not recover all original history or coordination records.

Keep a matched backup of the SQLite database, the document tree, and taskcli configuration. Include Obsidian's TaskNotes settings for the same display on another device. SQLite alone does not retain all published plan bodies or editable Notes. See [data coverage and recovery](../../../docs/task-board.md#data-coverage-and-recovery) for the field coverage and backup procedure.
