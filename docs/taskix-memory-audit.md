# Atomic memory audit

Audit date: 2026-10-06. Scope: extraction, write-time reconciliation, legacy
compaction, scheduling, retrieval boundaries, persistence and operator docs.
The architecture remains appropriate: durable atomic facts and immutable source
receipts, transactional reconciliation, asynchronous bounded model work, and
read-only effective-memory retrieval. The audit found and corrected four gaps.

## Findings and corrections

| Finding | Correction | Regression evidence |
| --- | --- | --- |
| Direct store mutations could publish an active value beside unresolved conflicting claims. | Check the final indexed state before every public mutation commits, as well as background batch commits. Keep intermediate batch snapshots from overriding final state. | `atomic_facts::direct_writes_cannot_publish_an_unresolved_fact`, `direct_conflict_transition_cannot_leave_a_confirmed_value`, `facts::tests::settled_check_uses_final_state_not_intermediate_batch_snapshots` |
| Every writable open replayed full-memory auxiliary backfills and replaced a trigger. | Atomic one-time auxiliary migration; preserve existing progress and leave ordinary opens free of those scans and schema invalidation. | `store::reopening_initialized_store_does_not_revisit_all_memory_rows`, `legacy_auxiliary_backfill_is_atomic_and_preserves_existing_progress`, ID/source migration suites |
| Human/retired and subsequently expired dirty rows caused repeated candidate scans. Suspended rows shared the candidate index. | Maintain dirty eligibility on writes, use a partial ready index, drain expired candidates in pages of ten without model calls, and avoid memory JSON in deadline probes. | `compaction::ineligible_revisions_do_not_accumulate_in_the_dirty_index`, `expiry_before_debounce_is_drained_without_model_work`, performance scale harness |
| A seed could expire during model execution and still commit. | Recheck expiry inside both normal and legacy-compaction commit transactions. | `compaction::compaction_commit_rechecks_seed_expiry_after_model_execution` |

Each behavioral correction was preceded by an observed failing regression.
[Performance measurements](taskix-memory-performance.md#atomic-memory-audit-2026-10-06)
include the same 1,000/10,000/100,000-record fixture before and after the candidate
index change. They measure local SQLite work, not model or full-daemon latency.

## Architecture and performance boundaries reviewed

| Requirement | Implementation and acceptance evidence |
| --- | --- |
| Independent facts and one current version | `Fact::key`, immutable-version validation, `memory_facts` unique active index; `atomic_facts` covers different qualifiers, concurrent writes, immutable values, replacement chronology and preservation of unaffected facts. |
| Human protection and source provenance | `consolidation` and `fact_compaction` validate target actor/revision and literal evidence within the transaction. Split retirement checks evidence coverage; source versions and one-to-many origins remain stored. Atomic-fact and consistency suites cover human targets, fabricated quotes and rollback. |
| Forgetting cannot be inferred from compaction | Explicit current user authorization and suppression of historical evidence; consistency and lifecycle regression suites. No history deletion is part of this audit. |
| No model calls under a database write transaction | Worker loads bounded snapshots, runs the model and validation feedback, then enters fenced store completion. `worker::model_loops_run_in_parallel_without_holding_database_transactions` verifies concurrent progress. |
| Concurrent and obsolete work cannot commit | Queue generation/owner/lease fences, source-revision cancellation, exact memory revision guards, commit-time expiry. Queue, worker and compaction suites exercise stale leases, retries, cancellation and old-seed wakeups. |
| No periodic idle compact scan | Dedicated notification/deadline loop in Taskix daemon, durable per-revision state, batch size ten. Daemon tests cover idle waiting, writes, restart/reload and pending work. Other service loops retain independent schedules. |
| Bounded model context and retries | AgentLoop checks serialized request size, tool/step limits and whole-loop timeout. Legacy split caps at four steps, sixteen parts and configured context bytes; quotation references preserve full evidence. DeepQuery caps at six steps, twelve calls, 64 KiB and sixty seconds. Agent-loop and atomic-fact tests cover oversized submissions, invalid feedback loops and cancellation. |
| Effective retrieval and historical diagnostics | Default list/FTS/vector/context paths filter active, unexpired records; conflict/history tools are separate. Store/context/vector/projection suites cover project isolation, lifecycle changes, stale caches and read-only projection. |
| Migration and publication durability | One transaction for migrations; versioned content and lineage survive; projection receipts repair failed publication. Migration, projection and source-index suites verify rollback, retries and restart. |

## Costs that remain explicit

- Exact vector ranking still visits the current Project's eligible vectors in
  256-row pages. Prior 10,000-vector release measurements support the present
  design; this is not an unlimited-scale ANN substitute.
- Broad lexical queries can visit many matching FTS entries. Result limits bound
  returned context, not all database work. Identity reconciliation uses the fact
  index instead of treating lexical search as a complete uniqueness check.
- Initial migration and full-service retained-source validation still scale with
  historical data. Removing repeated auxiliary backfills does not remove source
  integrity checks. Projection full reconciliation and explicit historical scans
  are separate maintenance operations.
- Exact fact snapshots are bounded to sixteen active/conflicted records. A large
  unresolved conflict set can require incremental or explicit resolution. The
  final index check refuses an active result while other conflicts remain; a
  truncated snapshot never establishes that all claims were resolved.
- Evidence validation and precedence reads are keyed by receipt but deserialize
  source content, potentially more than once for different quotations. Their
  cost follows selected source bytes and bounded proposal size, not total memory
  count. This audit does not claim constant-time writes for arbitrarily large
  receipts or measure remote provider throughput.
- Dirty revisions waiting behind existing pending/running work still require work
  state lookups. The published human-history benchmark does not measure an
  arbitrarily large waiting-work backlog.
- Model correctness is not proved by deterministic tests. Local mock acceptance,
  real Luna migration evidence and the original Gemini egress failure remain
  distinguished in [acceptance](taskix-memory-acceptance.md). This audit neither
  changes the production provider nor installs a new service binary.

The design, performance report and Wiki must describe these boundaries alongside
the automatic scheduling behavior. PR CI is the cross-platform gate; local
measurements alone do not establish Windows/Linux runtime performance.
