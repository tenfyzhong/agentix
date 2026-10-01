# Memory screening calibration (2026-09-29)

## Adopted policy and default

Taskix uses **negative screening** with
**`TASKIX_MEMORY_JEV_MIN_CONFIDENCE=0.75`** by default. Only a valid `skip` whose
confidence and selected probability both meet the threshold, with a margin of
at least `0.2`, suppresses extraction. All other outcomes continue to the model:
`extract`, uncertainty, low scores, malformed replies, disabled/invalid Jev
configuration and service failures. Source records remain stored after a skip.
The routing setting `TASKIX_JEV_MIN_CONFIDENCE` is independent and unchanged.
Explicit memory environment settings override the default; the rollback did
not modify installed/global configuration or deploy a service.

The positive-gate experiment was rolled back. The exact production question
from the successful negative-gate backtest was restored, not reconstructed from
memory. Its hash matches all four archived runs below, and replaying the restored
score rules reproduces every one of their 40 threshold rows. Rollback validation
used cached responses; no additional provider calls were needed.

## Context sent to Jev

Jev receives the full source message snapshot and the current extraction chunk.
The question identifies the current chunk as the decision target; other messages
only resolve references. It distinguishes disposable CI/test progress, delivery
receipts, routine operations, investigation plans, raw tool output, injected
execution boilerplate and pure repository-visible implementation summaries from
substantive choices, corrections, rationale, rejected alternatives, ownership
boundaries, external limitations, incidents and measurements. Mixed content with
useful knowledge remains eligible for extraction. No keyword hard skip or source
truncation is introduced. Missing references or incident evidence should remain
uncertain and therefore use the model.

## Data and evaluation method

A read-only local Agentix Project snapshot contained 146 Jobs and 6,534 messages.
The current memory implementation Job was excluded. Exact role/text duplicates
were removed before sampling, leaving 5,326 historical chunks. With seed
`20260929`, Jobs were partitioned before selecting 50 calibration and 50 holdout
chunks: **100 samples from 50 Jobs**, with 87 assistant and 13 user messages.
The current Codex main agent read and labeled all samples before viewing Jev
results: **77 skip, 23 extract**, each with a written reason.

A second set used seed `20260930` and excluded all previously sampled Jobs:
**50 samples from 35 different Jobs**, with 45 assistant and 5 user messages.
Codex again labeled these before seeing their Jev results: **40 skip, 10 extract**.
The revised question was frozen before this additional evaluation and was not
changed in response to it. Across both sets there are **150 distinct records
from 85 Jobs: 117 disposable and 33 worth extraction**.

Labels describe whether a record deserves repository-aware extraction, not
whether it must become a stored memory. They are one Codex reviewer's reference
judgments, not independent human ground truth. Original labels were never
changed to improve scores.

The original 100 samples received three real Jev runs; the additional 50 received
one run, for **350 live requests**. Model selector: `jev-latest` (a provider alias,
not a pinned model version). Each response was cached and replayed at exactly
**0.95, 0.90, 0.85, 0.80, 0.75, 0.70, 0.65, 0.60, 0.55, 0.50**. Thus the step is
**0.05**, with no finer-grid selection in this experiment. No service failures
or request-size fallbacks occurred.

Inputs follow the production historical backfill path: one source message per
record and chunks of at most 16,384 UTF-8 bytes, accompanied by the complete
message. Live multi-message turns were not benchmarked. Historical injected/tool
messages are retained in the sample as evidence of actual source data quality.

## Negative-screening results

Correct skips are measured against the 77 disposable original records and
40 disposable additional records. False skips lose extract-labeled records.

| Threshold | Original correct skips, run 1 | Run 2 | Run 3 | Original false skips, runs 1/2/3 | Additional correct skips / 40 | Additional false skips |
| --- | ---: | ---: | ---: | --- | ---: | ---: |
| 0.95 | 47 | 48 | 47 | 0 / 0 / 0 | 29 | 0 |
| 0.90 | 50 | 50 | 50 | 0 / 0 / 0 | 31 | 0 |
| 0.85 | 53 | 52 | 52 | 0 / 0 / 0 | 33 | 0 |
| 0.80 | 56 | 54 | 55 | 0 / 0 / 0 | 34 | 0 |
| **0.75** | **56** | **57** | **56** | **0 / 0 / 0** | **35** | **0** |
| 0.70 | 58 | 58 | 57 | 1 / 0 / 0 | 36 | 0 |
| 0.65 | 58 | 59 | 59 | 1 / 1 / 1 | 36 | 0 |
| 0.60 | 60 | 60 | 60 | 1 / 1 / 1 | 36 | 0 |
| 0.55 | 61 | 62 | 62 | 1 / 1 / 1 | 36 | 0 |
| 0.50 | 64 | 64 | 63 | 1 / 1 / 1 | 37 | 0 |

**Why 0.75:** require zero observed false skips in every original repeated run,
then maximize correct skips, preferring a higher threshold on a tie. `0.75`
provides the greatest coverage satisfying that requirement across repeated runs;
`0.80` loses coverage, and `0.70` already misses a reference record. The additional
50-record evaluation also observes no false skip at `0.75`.

At the adopted default:

- Original set: correctly filters **56–57/77 = 72.7–74.0%** of disposable records.
- Additional set: correctly filters **35/40 = 87.5%** of disposable records.
- Combining the first original run with the additional run: **91/117 = 77.8%**
  of disposable records filtered, or **91/150 = 60.7%** of all evaluated chunks.
- **All 33 distinct extract-labeled records are retained.** Repeated calls do not
  make these 33 records into a larger independent sample.

The lower-threshold error is original sample **s051**, a user's task-state
transition incident report with a missing screenshot. Codex conservatively
retained it for unresolved evidence, but Jev called it disposable. Its scores
passed `0.70` in one run and `0.65` in all three. The reference label and sample
were preserved, not revised or removed.

Before improving the context, the original broad question at its best tested
`0.50` setting filtered only 29–30 of the original 100 records. The adopted
context nearly doubles this to 56–57 at `0.75` while preserving all reference
memories in the measured sets.

## Why positive screening was rejected

A subsequent experiment allowed only sufficiently confident `extract` into the
model, skipping valid negative, uncertain and low-score answers. Across the
same 150 frozen labels and three runs (450 live requests), no tested threshold
from `0.95` to `0.50` retained every reference memory.

Even `0.50` correctly filtered 107 disposable records but consistently lost
**8/33 memories**, retaining only **25/33 = 75.8%**. Losses included user UI and
plugin-installation preferences, a transparent-proxy constraint, an external
daemon limitation, an incident with missing evidence, a presentation correction,
a timestamp/denominator policy and a local-versus-CI toolchain discrepancy.
Some were classified `extract` but failed confidence checks; others were
classified `skip`. Higher thresholds lost more memories. The improved filtering
rate therefore did not justify replacing negative screening. Positive behavior
and its `0.50` default are no longer active; experimental evidence remains local.

## Limits and provenance

This is a small, assistant-progress-heavy sample with only 33 positive records,
not proof of zero production memory loss. The original calibration/holdout split
became development evidence when its errors informed the revised context; the
additional 50 records supplied new evaluation evidence. Results do not establish
performance for other Projects, live multi-message turns or final memory quality.
Jev confidence is not a calibrated probability of memory loss.

Negative-run HTTP wall latency was approximately 902 ms median and 1,201 ms p95.
The harness uses an 8-second urllib socket timeout whereas production uses an
8-second total deadline; no timeout occurred here. These are observations, not
SLAs. A skipped benchmark chunk is not proof of committed production work or
measured token-cost savings.

Raw text, frozen labels/reasons, requests' question definitions, response sets,
checksums and reports remain private under the repository Git directory:

- `taskix-evaluations/20260929-memory-context-v2/`: original 100, three runs.
- `taskix-evaluations/20260929-memory-context-v2-fresh/`: additional 50, one run.
- `taskix-evaluations/20260929-memory-positive/`: rejected positive experiment.
- `taskix-evaluations/20260929-memory/`: initial pre-improvement baseline.

Adopted question SHA-256:
`464d69636ff733885dfcc761733089ab8cfe78dc7322c0ccf104ce606051d00e`.
Original sample SHA-256:
`c99fcdcfd74ee44fd61a2317947deabac0576cdcea1cbd718d52dd3ba71c183e`.
Original label SHA-256:
`00b548520e135887897dd29b883e814f011309ba39f5433d5f112d45a768ca39`.

## Reproduction and regression coverage

Use a new private output directory; the script overwrites output files:

```sh
python3 scripts/taskix-memory-eval.py prepare --output /private/evaluation \
  --project PROJECT_ID --exclude-job CURRENT_JOB_ID --count 100 --seed 20260929
# Independently label samples.json before viewing Jev output. labels.json format:
# {"s001": {"label": "skip", "reason": "Routine CI progress"}, ...}
# The following explicit live evaluation uses the existing Jev environment.
python3 scripts/taskix-memory-eval.py run --output /private/evaluation
python3 scripts/taskix-memory-eval.py report --output /private/evaluation
python3 -m unittest discover -s scripts/tests -p test_taskix_memory_eval.py
```

The current evaluator uses negative screening, verifies frozen artifact hashes,
and rejects positive-policy result files. Legacy unmarked runs are negative;
new runs carry `gate=negative_skip`. It loads the production question directly.
Regression coverage includes the exact 0.05 grid, confidence/probability/margin
gates, uncertainty and malformed-response fallback, independent memory threshold
and `0.75` default, plus real daemon/mock HTTP behavior. Deterministic tests use
no live credentials. Metric actions again mean confident skip/extract or agent
fallback; distinguish historical positive experiments by their time/policy window.
