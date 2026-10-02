# Agentix bridge implementation and validation

## Terminal interaction boundary

Attached Pi, OMP, and Claude Code sessions supply their original PID and client
identity to Agentix's shared [terminal interaction observer](../../docs/terminal-interactions.md).
Common choice lists and confirmations become IM requests; recognizable unsupported
dialogs produce a terminal handling notice. This does not add structured approval
events or capabilities to the bridge protocol. Claude's completion hooks and
optional Channel delivery retain their existing transport behavior.

## Native host validation

The native loader smoke tests have passed with Pi 0.84.4 and OMP 17.3.7. They exercise public extension APIs without model requests. Run them against installed hosts with:

```sh
AGENTIX_TEST_NATIVE_HOSTS=1 node --test plugins/agentix-bridge/tests/native-host.test.mjs
```

The default Rust and Node suites use local fixtures and do not need provider credentials or real IM accounts. See the [host protocol](../../docs/host-protocol.md), [layered architecture](../../docs/architecture.md), and [performance measurements](../../docs/performance.md) for implementation details.

## Native session replacement

`/new` asks the original client to replace its current session and lets Agentix follow the replacement. Pi invokes a private extension command with a command context; OMP uses verified rmux/tmux input; Claude maps to native `/clear` through its verified terminal. Active work is stopped before submitting the native command. Plain prompt delivery still rejects leading slash commands.

The bridge observes terminal session-switch lifecycle events as well. `client_id` stays stable across session incarnations; optional `previous_session_id` on replacement registration allows Agentix to recover a handoff after reconnect. Only a new-session lifecycle publishes that link; resume and fork do not. Agentix owns the short-lived handoff FIFO and persists it separately from the original session's queue.

When testing unpublished bridge changes in OMP, install this plugin directory with `omp plugin install ./plugins/agentix-bridge` from the repository checkout after removing any previously installed marketplace copy. Restart OMP to load the updated extension. Rebuilding or restarting Agentix alone does not update a bridge already loaded inside OMP. The packed plugin includes `new-session.mjs` for native session control.

OMP marketplace installations copy plugin files into a cache, even when the marketplace source is a local checkout. To refresh that installed copy after editing the checkout, run `omp plugin marketplace update agentix` followed by `omp plugin install agentix-bridge@agentix --force`, then restart OMP. Installing a marketplace copy replaces a previous local plugin link; check the actual installed path before assuming edits are loaded automatically.
