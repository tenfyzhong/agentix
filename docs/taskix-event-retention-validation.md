# Event retention validation

## Scope and reproduction

The reusable ignored Rust test `real_backup_retention_and_foreground_latency`
accepts `TASKIX_BENCHMARK_BACKUP`, an **offline SQLite online-backup artifact**.
It copies that file into a fresh temporary directory before opening Store. It
never opens or mutates the live database, and uses Store rather than Service, so
it cannot publish to the real document vault. Do not supply a raw copy of a live
WAL database: use the backup procedure in [Taskix backups](taskix-backup.md).

```sh
TASKIX_BENCHMARK_BACKUP=/path/to/offline/tasks.sqlite3 \
  CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
  cargo test -p agentix-task --lib real_backup_retention_and_foreground_latency \
  -- --ignored --nocapture
```

The test measures schema migration separately, creates a disposable benchmark
Project/Job, measures 200 updates to that same Job without cleanup, then measures
200 updates while a background worker compacts the original history. SHA-256
checks cover canonical Project, Job, Task, Plan, lease and Inbox rows before/after
migration and cleanup (excluding only benchmark entities). The test also runs
SQLite `integrity_check` and reports durable maintenance progress. Temporary copies
are removed when the test exits. No event or conversation contents are printed.

These are Store write timings, not complete CLI startup, document projection,
network delivery, cold-disk timings, or production latency guarantees. Measurements
are a single local run with Rust 1.95's unoptimized test profile on macOS/APFS;
200 samples are useful for regression investigation, not a service-level promise.

## Local result, 2026-10-01

The offline source was schema 14, 564,899,840 bytes (538.7 MiB), with 15,718 events.
All source events were inside the default 30-day retention window. This exercises
legacy snapshot compaction and page reclamation, rather than expiry deletion.

| Measurement | Result |
| --- | ---: |
| First schema/auto-vacuum migration | 16.330 s |
| Background backlog drain, including concurrent writes | 18.991 s |
| Final database after checkpoint | 55,230,464 bytes (52.7 MiB) |
| Size reduction | 90.2% |
| Update P50, worker absent / active | 0.919 / 0.966 ms |
| Update P95, worker absent / active | 1.031 / 1.102 ms |
| Update P99, worker absent / active | 2.321 / 2.470 ms |
| Maximum update, worker absent / active | 3.791 / 3.477 ms |
| Canonical row checksums | Unchanged |
| SQLite integrity check | `ok` |

The compaction cursor reached the captured legacy boundary (15,718), pruning
watermark remained zero, and the next daily cycle was scheduled. The final size
includes the temporary benchmark entities and their new events. Free pages below
the incremental threshold and partially filled pages can remain.

The first upgrade is intentionally a distinct operation: enabling incremental
vacuum for an existing SQLite database requires a one-time rebuild. Reserve spare
disk space and a writer maintenance window when upgrading all writers. It is not
included in the steady-state update percentiles and is not repeated daily.

## Automated regression coverage

- Preview versus apply, transaction rollback and canonical-state preservation.
- Persistent global/Project sequence watermarks after pruning and migration.
- Age-index query plans, retention boundary, disabled policies and restart progress.
- Worker file-lock ownership, foreground write-lock avoidance and asynchronous dispatch.
- Detached CLI worker continuation after the initiating command exits.
- Row/byte batch limits, versioned UTF-8 string limits, unsupported payload markers
  and idempotent summaries.
- Incremental page reclamation and concurrent worker serialization.
- Existing Agentix task notification integration without changes to Agentix consumers.
