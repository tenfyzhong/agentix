# Configuration and Operations

The user guide has moved to the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki/Configuration-and-Operations). Start with the [Wiki home](https://github.com/tenfyzhong/agentix/wiki/Home) for installation, configuration, and everyday use.

The implementation and development notes below remain in the repository.

### CI test cost

Plugin tests run in parallel with Rust tests on each supported operating system. CI disables dev/test debug symbols to reduce Windows linker work and cache size; local Cargo profiles are unchanged. Windows retains the workspace check, native TCP control tests, task-board tests, and three system-time-zone checks. Compare GitHub Actions step timings on equivalent revisions and cache states before claiming a speedup; the baseline Windows run `34441426541` took 17m26s, including 3m43s for workspace checking, 4m31s for the TCP test step, and 5m57s for task-board tests.

## Codex proxy verification

The reusable test suites cover the following boundaries:

| Boundary | Coverage |
| --- | --- |
| Actual `serve` startup | Active and stale Unix sockets, regular files, and occupied WS ports fail with error logs, preserve existing paths, and do not launch the upstream |
| Transport | Unix and WS listeners, WS upstream, stdio JSON lines, independent client request IDs, unchanged text frames, server requests, and streamed notifications |
| Lifecycle | Client and upstream disconnect cleanup, unsubscribe, multiple owners, socket permissions and replacement inode protection, detached upstream survival and reuse |
| Discovery | Registry-backed listing, sessions without rollout files, stale attach rejection, and PID-to-terminal association |

Run `cargo test -p agentix-codex -p agentix --lib --tests` and `cargo clippy -p agentix-codex -p agentix --all-targets -- -D warnings`. The ignored subprocess fixture is invoked by its parent integration test. Run the optional allocation-path timing comparison with `cargo test -p agentix-codex --test proxy_registry benchmark_stream_notification_observation -- --ignored --nocapture`; it has no timing threshold and is not an end-to-end throughput benchmark.
