# Obsidian editing synchronization

Taskix Sync waits for **five idle seconds** before synchronizing saved Issue or
Milestone body edits. Each edit restarts that note's timer. Human status changes
keep their separate 300 ms delay.

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
