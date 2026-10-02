# Terminal interaction recognition and lifecycle

Agentix observes attached Codex, Pi, OMP, and Claude Code sessions through the
configured tmux or rmux driver. Recognition depends on dialog structure rather
than slash commands, prompt titles, or option names. A future command that uses a
supported dialog shape follows the same IM response flow.

## Supported shapes

- Numbered single-choice lists with a selected `›`, `❯`, or `>` row.
- Arrow lists with a selected `→` row and explicit navigation, selection, and
  cancellation hints, including standard Pi extension selectors.
- Explicit `[y/N]` or `(y/n)` confirmations at the live cursor.

Lists must contain 2–20 complete visible options. Selection responses use the
default Up/Down, Enter, and Escape controls; confirmations use `y` or `n`.
Custom keybindings and paginated, filtered, wrapped, or multiple-selection
layouts are not guaranteed to match these shapes.

Agentix presents the available choices and waits for an explicit response. It
does not submit the highlighted default. `/cancel` cancels a choice list or
declines a confirmation. Ordinary terminal output and composer drafts do not
create interaction requests.

A dialog with recognizable selection/cancellation hints but an unsupported
shape is shown as **Terminal needs attention**, with the captured terminal text
and original pane ID. Use `/rmux` or `/tmux` to locate that pane, handle the dialog
there, and dismiss the IM notice with **Handled in terminal**. That button never
sends terminal input. Arbitrary custom UI without recognizable dialog markers
cannot be detected reliably; the original terminal remains its fallback.

## Observation and response lifecycle

`AgentRegistry` observes only attached sessions whose native adapter supplies a
live original PID and client identity. The configured terminal port performs
read-only inspections; two observations of the same dialog are required before
publishing controls. An existing structured interaction takes priority over its
terminal fallback. A dialog is published once until its contents change or it
closes. Native resolution, detach, changed client identity, and changed dialog
contents invalidate old controls.

Responses are scoped to the original backend, session, client, PID, and pane.
Before input, the shared terminal adapter rechecks the foreground process, pane
state, and dialog identity. Copy mode, closed dialogs, changed content, or a
different pane reject the response. Navigation rechecks the dialog after each
step and confirms the requested row twice before Enter. A response is consumed
once; failed or uncertain submissions are not automatically replayed.

IM-initiated new-session transitions keep their startup deadline separate from
human decision time. Waiting on a native dialog does not expire the transition;
after the response, the replacement startup deadline resumes. Cancellation or a
failed terminal response reports a failed transition rather than choosing an
alternative automatically.

Observation controls are held in memory. After an Agentix restart, attached
sessions can rediscover a still-visible dialog; controls from the previous
process remain invalid. Terminal capture is required, so sessions outside the
configured multiplexer have no terminal fallback. Structured Codex approvals and
questions retain their existing protocol transport and free-text answer flow.

## Validation

Reusable parser tests cover unfamiliar titles, arbitrary options, numbered and
arrow selectors, confirmations, unknown dialogs, and ordinary output. Isolated
native fixtures verify explicit choices, cancellation, changed dialogs, copy
mode, foreground changes, and absence of input during inspection.

Registry tests exercise every backend, reply routing, duplicate responses,
identity changes, detach, protocol priority, and local dismissal. Codex-to-engine
tests retain new-session handoff, deadline, cancellation, and no-model-prompt
assertions with the shared observer enabled. The opt-in real Codex test exercises
the same generic inspection and response functions through both tmux and rmux.
Live Pi/OMP/Claude dialogs and live IM transport remain separate acceptance work.
