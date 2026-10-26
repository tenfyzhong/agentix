# Obsidian editing synchronization

Taskix Sync waits for **five idle seconds** before synchronizing saved Issue or
Milestone body edits and Issue `title` property edits. Each edit restarts that
note's timer. Human status changes keep their separate 300 ms delay.

Issue delivery fingerprints include `title`, so a property-only edit reaches the
same import path as body edits. The importer updates the database title and
readable name together under the note revision guard. Existing name normalization,
collision handling, and numbered filename rules apply. Publication refreshes the
leading H1 and related links while preserving authored sections, including their
own headings. Blank, non-text, multiline, and stale title edits are rejected
without replacing the saved note. A pending database publication retains priority
over its previous projected title.

Renaming an Issue file in its existing directory also queues delivery. When the
registered path is missing, the importer locates the single managed note with the
same immutable ID and Project, checks its revision, and derives the title from
the filename after the existing date/sequence prefix. The new name must already
satisfy portable naming rules (at most 48 characters). Ambiguous copies, conflicting
property edits, changed prefixes, and name collisions reject import. Previewing
the normal mutation ensures its resulting path matches the moved source before
committing; publication can recover that source even when the registry still
contains the old path. Pending Issue publication performs the same reconciliation
before writing, preventing it from recreating the old note over an unimported
rename. The source note's authored sections remain intact.

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
