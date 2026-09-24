# Project resolution performance

## Architecture and invariants

`ProjectDirectory` separates filesystem/Git discovery from persistence. CLI implicit selection may register a non-Git directory; IM lookup remains read-only. Both use the same indexed service lookup. A `context` command lazily discovers identity once and reuses it after Inbox synchronization, while re-reading database state so cancellations and ownership changes remain visible. Explicit Project selection and owned work do not need directory discovery. Git remotes are queried only for explicit registration.

Schema 14 maintains `project_lookup` in the same transaction as Project writes, with cascading deletion. Canonical roots and Unicode-lowercased keys are indexed independently. The migration builds these indexes once from existing Project metadata; ordinary startup does not rebuild them. Existing Project records and document paths are preserved. Root lookup materializes at most one Project, with the earliest legacy record winning if multiple stored aliases identify the same canonical directory.

Directory lookup has O(log P) index cost, independent of unrelated Project bodies and filesystem paths. Name allocation performs indexed equality checks for the base name and occupied numeric suffixes, O((C + 1) log P) for C collisions. It preserves the existing first-free-suffix and Unicode case-folding behavior. Registration still incurs a transaction and document publication; that necessary cold-path cost is not hidden by a stale cross-process cache.

## Reproduce

Use Rust 1.95.0 and run the reusable ignored integration benchmark:

```sh
cargo test -p taskix --test cli project_resolution_scaling_benchmark -- --ignored --nocapture
```

The benchmark creates isolated databases and real temporary directories, seeds 1, 1,000, or 10,000 unrelated Projects, and executes the real `taskix context` binary 20 times per scenario. `warm` reads the last seeded directory repeatedly. `cold` uses a new directory for each sample, including registration and document publication. Each database also contains the fixture's seed Project. Setup and seeding are excluded from the measured interval; CLI startup, discovery, database work, and projection work are included. Median is sample 11 and P95 is sample 19 after sorting 20 observations. These are diagnostic reports, not timing assertions in CI.

## Measured comparison

2026-09-24, Apple M3/macOS, Rust 1.98.1 debug build (Cargo 1.95.0 with the compiler selected from the host PATH), macOS 26.5 SDK. Before is PR #115 commit `ed3a832`; after uses the indexed implementation and the same benchmark harness. Times are milliseconds from one run per implementation and include local scheduling noise. Both sides used the same toolchain combination. Functional and lint validation is separately run with the entire Rust 1.95.0 toolchain directory first on PATH, matching CI.

| Unrelated Projects | Scenario | Before median | After median | Before P95 | After P95 |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | Repeated | 28.653 | 21.834 | 75.123 | 30.937 |
| 1 | First visit | 46.824 | 31.475 | 77.072 | 51.829 |
| 1,000 | Repeated | 51.223 | 16.743 | 71.678 | 20.807 |
| 1,000 | First visit | 88.435 | 33.961 | 93.178 | 67.420 |
| 10,000 | Repeated | 333.515 | 17.769 | 456.491 | 40.335 |
| 10,000 | First visit | 558.009 | 30.107 | 762.731 | 31.702 |

At 10,000 Projects, median latency fell by about 94.7% for repeated visits and 94.6% for first visits. The scaling trend matters more than absolute millisecond values. These measurements do not establish production latency, cold OS-cache latency, network-mounted vault performance, or universal optimality.

## Regression coverage

Normal CI asserts functional and structural invariants: a single Git discovery with no remote lookup per context (Unix shim test), real Git/worktree/subdirectory behavior, unrelated malformed Project isolation, indexed query plans, schema migration, path aliases, Unicode name collisions, deletion, concurrent registration, and failed projection recovery. Existing lease, Inbox cancellation, session history, and host-plugin tests run against the same implementation. The manual benchmark supplements these deterministic checks; it does not replace them or claim 100% line coverage. Symlink and process-count assertions run on Unix; shared CLI/storage behavior runs on Linux, macOS, and Windows.
