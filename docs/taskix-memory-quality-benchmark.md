# Project memory quality benchmark

Status: raw-dialogue retrieval baseline measured; extraction and end-to-end
experiments are in progress. The synthetic timings in `taskix-memory-performance.md` are
not evidence of extraction or retrieval quality.

## Scope

Evaluate the real Taskix extraction/consolidation worker and Rust retrieval
implementation. Compare FTS alone with FTS plus Ollama BGE-M3. Keep ingestion,
retrieval, and answer quality separate so that a stronger answer model cannot
hide missing memories. Preserve the product contract: repository-visible facts,
unconfirmed proposals, and transient progress must not become project memory.

## Public corpus and provenance

Use the complete [LoCoMo ACL 2024 release](https://github.com/snap-research/locomo)
by Maharana et al., pinned to commit
`3eb6f2c585f5e1699204e3c3bdf7adc5c28cb376`.
The SHA-256 of `data/locomo10.json` is
`79fa87e90f04081343b8c8debecb80a9a6842b76a7aa537dc9fdf651ea698ff4`.
The release contains 10 conversations, 5,882 dialogue turns, and 1,986 questions:

| Category | Questions |
| --- | ---: |
| 1 | 282 |
| 2 | 321 |
| 3 | 96 |
| 4 | 841 |
| 5 (adversarial) | 446 |

The non-adversarial denominator is 1,540. Report adversarial abstention separately;
do not silently omit it from the evaluation. Missing or invalid evidence IDs
must be counted and disclosed rather than treated as retrieval failures or hits.
The upstream corpus is CC BY-NC 4.0. Download it separately for research; do not
vendor it into the software distribution. Retain upstream attribution and license
alongside local artifacts.

LoCoMo is generated persona dialogue with human annotations, not a software
project decision dataset. Treat its two speakers as separately attributed source
speakers, preserving dates and dialogue IDs. Neither speaker is an assistant
whose proposals were approved by the other. Only conversation text and supplied
image captions enter ingestion; exclude QA, gold evidence, event summaries,
generated observations, and session summaries. Gold fields are scoring inputs
only. An empty fixture repository represents unavailable external facts; it does
not establish performance on repository filtering.

Add an independently authored project-memory suite covering explicit decisions,
rejected alternatives, conditional scope, corrections, conflicting assertions,
repository-known facts, unconfirmed suggestions, secrets, and transient status.
Include Chinese and English. Freeze expected memories and forbidden memories
before model execution. Report this suite separately from LoCoMo.

## Development and held-out protocol

Before any scored run, assign `conv-26` and `conv-30` to development and the other
eight conversations to held-out evaluation. Do not select tuning parameters using
held-out answers or failures. Final tables include development, held-out, and
complete-corpus scores with their exact denominators. An all-corpus score is not
an independent held-out score. Retain the original baseline before optimization.

Use paired question-level comparisons and confidence intervals; also report
conversation-level variation because questions within a conversation correlate.
Record every tuning candidate and its development score. Freeze the chosen
configuration before the final held-out run. Any subsequent inspection-driven
change requires an explicit exploratory label, not a renewed held-out claim.

## Evaluation layers

1. **Retrieval isolation:** index the same raw dialogue units in all retrieval
   variants. This measures ranking, not successful extraction. Use actual Taskix
   FTS and hybrid code, plus dense-only as a diagnostic where supported.
2. **Extracted memory:** replay all source conversations through extraction and
   consolidation, then search the resulting memories. Preserve provenance and
   report rejected candidates, errors, retries, compression, and evidence coverage.
3. **Answering:** apply one fixed reader and grading protocol to each system's
   retrieved context. Use the same context budget and top-k cutoffs. Report token
   F1 and semantic correctness separately, with abstention on adversarial items.
4. **Project policy:** score supported required facts and forbidden memory
   promotion on the supplementary suite. Retrieval improvements must not relax
   repository exclusion or source-attribution checks.

Retrieval metrics include evidence recall, all-evidence coverage, hit rate, MRR,
and nDCG at 5, 10, and 20. Deduplicate source IDs when multiple memory records
cite one turn. Evidence presence measures coverage, not whether an extracted
memory faithfully preserves the answer; assess that separately. Latency includes
query embedding for hybrid retrieval; also report warm cached and uncached
requests separately. Record indexing time, model calls, token usage when exposed,
and database size.

## Models and comparison systems

Use real [BGE-M3 from Ollama](https://ollama.com/library/bge-m3), recording its
manifest digest, dimensions, Ollama version, and embedding input format. Run an
isolated local Ollama instance so benchmark setup does not alter production
services. Extraction, answer, and judge model identities must be recorded exactly;
do not silently fall back to an older model or fabricated responses.

Use [Mem0's official benchmark implementation](https://github.com/mem0ai/memory-benchmarks),
pinned to `4b61c5d31b9c668a12b4f5e78064248a02c82d2b`, to inspect its ingestion and
scoring protocol and establish a reproducible OSS comparison. Match dataset,
model, embedding, reader, and context budgets where possible. List unavoidable
differences explicitly. Published Mem0, Zep, or other system scores belong in a
separate reference table with source, model, denominator, and metric; they cannot
establish a direct win against a locally measured result.

## Completion evidence

The final report must contain reproducible commands, pinned dependencies,
per-question artifacts, error/coverage manifests, extraction and retrieval scores,
baseline-versus-optimized results, held-out results, and other-system comparisons.
Every run must finish with all expected records accounted for. A partial run,
successful harness unit tests, or a smoke test does not satisfy this benchmark.

## Environment validation

Initial environment checks (2026-09-30):

- Ollama 0.34.2, isolated endpoint `http://127.0.0.1:11435`.
- BGE-M3 F16, 566.70M parameters, 1,024-dimensional embeddings, model digest
  `7907646426070047a77226ac3e684fbbe8410524f7b4a74d02837e43f2146bab`.
- A two-input English/Chinese embedding request returned finite vectors of the
  expected dimension. This is a connectivity check, not a retrieval score.
- The installed Codex CLI can access `gpt-6-astra` using its saved authentication.
  An ephemeral, read-only connectivity request returned the expected JSON.
  The invocation used `--ignore-user-config` and made no tool calls. CLI overhead
  was 19,871 input tokens even for this tiny request; account for this separately
  and investigate a lower-overhead adapter before full extraction replay.
- No independent OpenAI API key was available in the checked configuration.
  Authentication secrets remain outside benchmark artifacts and repository files.

## Reproduce the raw-dialogue retrieval baseline

From the repository worktree, with the pinned upstream dataset downloaded and
Ollama serving BGE-M3 on port 11435:

```sh
python3 -m unittest discover -s scripts/tests -p test_memory_benchmark.py
cargo test -p agentix-memory --example retrieval_benchmark
python3 scripts/memory_benchmark.py prepare /path/to/locomo/data/locomo10.json /path/to/run
python3 scripts/memory_benchmark.py embed /path/to/run/corpus.json /path/to/run/vectors.jsonl
python3 scripts/memory_benchmark.py embed /path/to/run/questions.json /path/to/run/vectors.jsonl
cargo build --release -p agentix-memory --example retrieval_benchmark
target/release/examples/retrieval_benchmark /path/to/run/corpus.json /path/to/run/questions.json /path/to/run/vectors.jsonl /path/to/run/rankings.jsonl
python3 scripts/memory_benchmark.py score /path/to/run/questions.json /path/to/run/rankings.jsonl /path/to/run/scores.json
```

Use a new rankings filename for each run. Passing `-` instead of the vectors file
runs FTS alone; score that file with `--modes fts`. Embeddings are computed from
source text or the question only, never serialized gold annotations. The adapter
uses a temporary isolated database and does not mutate the user's memory store.
The raw baseline imports each dialogue as a human-authored record solely to
isolate retrieval; this deliberately bypasses extraction and is not an extraction
quality result. Its `retrieval_ms` excludes precomputed query embeddings and must
not be reported as end-to-end hybrid latency.

The initial dataset audit found nine questions with at least one nonexistent
source ID. The scoring export retains their IDs and invalid references. They are
excluded from evidence-ranking aggregates, as are adversarial and empty-evidence
questions; all questions still receive retrieval results. The scorer rejects
missing, duplicate, or unexpected result rows before producing aggregates.

Initial FTS development baseline (230 scorable questions): evidence recall@10
58.38%, hit@10 63.48%, all-evidence coverage@10 54.35%. These are raw-dialogue
retrieval metrics, not answer accuracy. Held-out results are saved but have not
been inspected for tuning. The complete hybrid baseline also finished: 1,986 questions each produced FTS
and hybrid rankings (3,972 result rows), with no missing or duplicate rows.
On the same 230 development questions:

| Mode | Recall@5 | Recall@10 | Recall@20 | All evidence@10 | MRR@10 |
| --- | ---: | ---: | ---: | ---: | ---: |
| FTS | 50.20% | 58.38% | 64.94% | 54.35% | 0.4172 |
| FTS + BGE-M3 | 54.58% | 63.25% | 69.99% | 58.26% | 0.4402 |

Both use one dialogue turn per record, date/speaker attribution, and the supplied
image captions. The existing reciprocal-rank fusion uses a constant of 60 and
80 candidates per channel for the requested top 20; @5 and @10 here are prefixes
of that top-20 ranking, not separate production queries at those limits.
Extraction, reader, optimization, and competitor results remain outstanding.
