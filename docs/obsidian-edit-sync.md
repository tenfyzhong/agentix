# Obsidian editing synchronization

Taskix Sync waits for **five idle seconds** before synchronizing saved Issue or
Milestone body edits and Issue `title` or `issue_type` property edits. Each edit restarts that
note's timer. Human status changes keep their separate 300 ms delay.

Issue delivery fingerprints include `title` and `issue_type`, so a property-only edit reaches the
same import path as body edits. The importer updates the database title and
readable name together under the note revision guard. Existing name normalization,
collision handling, and numbered filename rules apply. Publication refreshes the
leading H1 and related links while preserving authored sections, including their
own headings. Blank, non-text, multiline, and stale title edits are rejected
without replacing the saved note. A pending database publication retains priority
over its previous projected title.

The importer also persists `issue_type` changes under the note revision guard.
Supported values are `feature`, `bug`, `enhancement`, `documentation`, `question`,
and `refactor`; an empty, null, or removed property clears the type. Invalid
values and stale revisions reject import without overwriting the note.
Targeted synchronization also rejects type changes on terminal Issues; reopen
a terminal Issue before changing its type. Full synchronization restores terminal
Issue types from the database, including invalid property values, so a rejected
type edit cannot block synchronization of unrelated documents. It preserves the
terminal status and does not update the stored type or revision merely to repair
the property.
Pending database publication takes priority over previously projected type values.
Repeated synchronization of an unchanged type does not advance the revision.
The plugin's separate status reconciliation excludes `issue_type` from restored
database properties, preserving pending type edits during status or Milestone
checks and reconnect recovery until the delivery importer validates them.

Renaming an Issue file in its existing directory also queues delivery. When the
registered path is missing, the importer locates the single managed note with the
same immutable ID and Project, checks its revision, and derives the title from
the filename after the existing date/sequence prefix. The new name must already
satisfy portable naming rules (at most 48 characters). Ambiguous copies, conflicting
property edits, changed prefixes, and name collisions reject import. Previewing
the normal mutation ensures its resulting path matches the moved source before
committing; publication can recover that source even when the registry still
contains the old path. Pending Issue publication performs this reconciliation only when the canonical
file is missing, preventing it from recreating the old note over an unimported
rename. Ordinary status publication does not import existing Issue prose or
invalidate a pending review. The source note's authored sections remain intact.

The plugin records pending document paths in vault-scoped local storage before
starting the debounce timer; it does not store another copy of the prose. After
reconnection or plugin reload it rereads those notes, including closed notes, and
requeues synchronization. A path is acknowledged only after successful delivery
of the current edit. Unloading cancels timers and subprocesses but retains pending
paths. Malformed sections or synchronization failures retain the pending work and
show an error. Clearing Obsidian's local storage removes this recovery queue;
opening/editing the note or running `taskix obsidian sync ID` reconciles it again.
The note itself always retains saved edits while synchronization is pending.

Memory notes remain read-only. Their edit check also waits for five idle seconds
before restoring authoritative database content; reopening a Memory note checks
it again. The read-only warning and restoration errors remain visible.

The plugin filters only the exact English native notice
“... has been modified externally, merging changes automatically.” for verified
Taskix-generated documents inside its configured active/archive directories.
Saving and automatic merging are unchanged. Errors, notices about unmanaged
notes, and other localized messages remain visible. Observers cover the main
window and existing/new popout windows and disconnect when windows close or the
plugin unloads. This filtering does not claim to detect or resolve merge conflicts.

Update and reload the Obsidian Taskix Sync plugin to enable these behaviors.
