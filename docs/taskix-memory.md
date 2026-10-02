# Taskix project memory

The user guide has moved to the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki/Taskix-Memory). Start with the [Wiki home](https://github.com/tenfyzhong/agentix/wiki/Home) for installation, configuration, and everyday use.

The implementation and development notes below remain in the repository.

## Verification and limits

See [memory acceptance](taskix-memory-acceptance.md) for reproducible commands,
coverage and measured scale results. Tests use real SQLite, local IPC, actual CLI
processes, host hooks and filesystem projections. Model/embedding HTTP responses
are mocked; this verifies contracts and recovery, not live provider availability,
semantic extraction quality or a production latency guarantee. Repository search
is bounded and reports incomplete coverage. Secret filename filtering and model
instructions reduce accidental capture but are not a content-classification
security boundary; only enable providers approved for the source material.

For a real Job-data comparison against frozen Codex reference labels, see the
[memory screening calibration](taskix-memory-calibration.md). Its empirical
recommendation is independent of routing confidence. The revised screening
context uses a `0.75` default; explicit service environment settings still override it.
The positive-gate experiment was rolled back because it missed reference memories.

## Derived-data retention and performance checks

Background maintenance runs every minute. It scans at most 1,000 rows per derived
table per pass using persisted keyset cursors, so large backlogs drain over
multiple passes and resume after restart. It removes vectors and failure records
that no longer match a searchable memory revision and the current embedding
generation. Current retry state is retained.

Cancelled work without a work audit or repository-review reference is eligible
for deletion 30 days after maintenance first observes it. Upgrades start this
observation period instead of immediately deleting historical cancellations.
Pruned work IDs no longer appear in work diagnostics or queue counts. Original
sources, source heads, memory versions, suppressions, decisions, work audits,
and reviewed work are retained. Done/failed work is not automatically pruned.
SQLite reuses freed pages; this does not promise immediate file-size reduction
and does not run automatic VACUUM.

See [performance measurements](taskix-memory-performance.md) for repeatable
local benchmarks and their scope. Exact vector retrieval remains enabled;
the measured 10,000-record, 384-dimensional Project does not justify adding an
approximate index. Higher dimensions and larger datasets require fresh profiling.

Startup validates every retained source against the task database, using one
bounded primary-key query per page of at most 100 receipts. Full content and
instance checks remain mandatory; total startup work still grows with retained
history size. This reduces query round trips without claiming a measured speedup
or changing exact vector retrieval into approximate nearest-neighbor search.
