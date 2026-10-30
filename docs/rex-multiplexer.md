# Rex multiplexer

Set the service configuration to select Rex explicitly:

```toml
[multiplexer]
kind = "rex"
```

The default `auto` selection probes rmux, then tmux, then Rex. Explicit `rex` never falls back to another backend. Start Rex before Agentix and restart Agentix after changing the configuration or starting a previously unavailable server. Probes never start a daemon.

Rex is supported through a local Unix socket on macOS and Linux. Executable discovery checks absolute PATH entries first. On macOS it also checks Rex.app and Rex Beta.app under `/Applications` and `~/Applications`. No command-path configuration is required. Remote Rex hosts and Windows terminal control are not supported.

The `/rex` menu supports existing panes, new sessions, new windows, horizontal and vertical splits, and the existing agent selection flow. Window selection shows one named button per window, without a separate numbered list. Launches run `cd -- <directory> && <agent> <arguments>` in the pane's original interactive shell. Exiting the agent returns directly to that same shell; no wrapper or replacement shell is added. Session and window names must not begin with `-`. New windows and splits preserve the current focus.

Codex uses shared draft protection, native `/new`, explicit checkout selection, `/exit`, and supported interaction recognition. Claude terminal delivery uses native paste events; its `/new` uses `/clear`. OMP uses the shared native session-control path. Other agent capabilities retain their existing bridge protocols. The bridge identifies Rex panes using inherited `REX_SESSION` and `REX_BLOCK`, verifies local ownership and process ancestry, and pins the full IDs and local endpoint. An ambiguous terminal, changed foreground process, unsupported screen, or uncertain mutation fails without trying another backend. If creation succeeds but its result cannot be verified, inspect Rex before retrying; Agentix does not repeat the mutation automatically.

Snapshots decode Rex VT output with bounded dimensions and pagination, retaining cursor position, wide graphemes, background colors, and dim placeholders needed by existing parsers. Unsupported control sequences fail closed. Customized themes and changed upstream terminal layouts can still require manual interaction in the original pane.

## Verification

Unit tests cover endpoint validation, executable discovery, local block filtering, process ancestry, launch arguments, paginated UTF-8 output, wide graphemes, and shared transport behavior. Node tests cover ownership rejection, native paste, draft preservation, and uncertain submission.

Opt-in tests create isolated sessions on an already running local Rex service and clean up only those sessions:

```sh
cargo test -p agentix-rex --all-features -- --include-ignored
cd plugins/agentix-bridge
AGENTIX_TEST_REX=1 node --test tests/rex-native.test.mjs
```

Native tests exercise single-EOF return to the original shell PID with a quoted Unicode directory, session creation, splitting, snapshots, a deterministic Codex composer with draft confirmation and checkout selection, and a deterministic Claude composer with multiline Unicode paste. They do not contact a model or verify live IM delivery. Local macOS acceptance used Rex CLI `0.0.1793047680-g2db882` (API revision 7) against server `0.0.1792787366-g623b2d`; Linux and real-agent end-to-end acceptance remain separate checks.
