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

## Extraction replay progress and reproducibility

`prepare` also exports `sources.json`: one immutable receipt per original dialogue
turn, preserving speaker/date text and the dialogue ID as its message ID. Both
persona speakers use the `user` evidence role; attribution remains in the text.
All receipts for a conversation share a session so backward source discovery
works. Synthetic receipt timestamps establish order only; original event dates
remain in the source text. No gold QA fields enter these receipts. The initial
export contains all 5,882 turns; this does not mean extraction has completed.

The independently authored policy fixture is
`crates/agentix-memory/tests/fixtures/quality/policy-cases.json`. Its first frozen
version contains 12 cases and has SHA-256
`1e6e040bd4303d5e125ca3b0116f504d9f883b493beba3e775982d92f1baefaf`.
Required and forbidden facts are grading inputs, excluded by `prepare-policy`.
This small development suite tests policy boundaries; it is not an estimate of
real-world extraction accuracy. The offline case is related to the development
smoke case and is not an independent held-out example.

```sh
python3 scripts/memory_benchmark.py prepare-policy crates/agentix-memory/tests/fixtures/quality/policy-cases.json /path/to/policy-sources.json
cargo run -p agentix-memory --example extraction_benchmark -- /path/to/policy-sources.json crates/agentix-memory/tests/fixtures/quality/repository /path/to/new-policy-run
```

The replay adapter requires the installed Codex CLI with access to `gpt-6-astra`.
It executes the production worker and repository/source tools, saves each model
request, response, event stream and stderr, and retains the resulting isolated
SQLite database. Start with a new output directory. Append `--resume` to the same
command after an interruption to continue an existing run. The adapter holds an
exclusive OS file lock, validates source/repository/binary/model/CLI fingerprints,
and checkpoints after draining each receipt. Replayed intake is idempotent, and
request numbers resume above all existing artifacts. Keep the exact executable
used for the run; a rebuilt executable with a different digest requires a new run.
A failed worker remains a failed run and is not silently skipped on resumption.
CLI-native tool events invalidate a model-only benchmark attempt. Only the outer
production worker may execute the returned memory/repository tool calls.

Initial smoke `extraction-smoke-v1` failed all three worker attempts because the
model requested an empty repository query. The runtime already prohibited it,
but the tool schema did not disclose that restriction. The schema now specifies
a nonempty query and describes the 512 UTF-8 byte limit; the existing runtime
byte check remains authoritative. Regression testing reproduced the missing
schema constraint before the fix.

`extraction-smoke-v2` completed one extraction and one consolidation, with no
failed work and one active memory. Inspection confirmed the Acorn production
scope, contractual reason, rejected cloud embeddings, staging exception, and
literal source evidence. All six model event streams contained no CLI-native
tool actions. Their aggregate usage was 124,732 input tokens (30,464 cached) and
627 output tokens. This verifies the pipeline on one development example only;
CLI overhead is substantial and must be addressed before an economical full
replay. The subsequent completed policy-suite results are recorded below.


The model-only adapter now uses the documented
[`model_instructions_file`, tool feature flags, and `web_search` configuration](https://developers.openai.com/codex/config-reference/)
to replace general coding instructions and omit shell, collaboration and web
search tools for that invocation. It still supplies the unmodified production
memory instructions, history, and tool schemas. Global configuration and saved
authentication are unchanged.

On the same development smoke input, `extraction-smoke-v3` completed extraction
and consolidation with one active memory, no failed work, and preserved the
contractual reason, rejected cloud embeddings and staging exemption. Six requests
used 81,270 input tokens, compared with 124,732 for v2 (34.84% fewer). This is an
observed single-run adapter comparison, not a general quality or latency claim.
The remaining approximately 13,000 input tokens per request still include CLI
context; include them in the actual cost accounting.

Full development-conversation extraction is running separately for `conv-26`
(419 receipts) and `conv-30` (369 receipts), with one isolated database per
conversation. The executable is pinned in the external artifact directory before
launch. These are whole conversations, not a sampled subset of their turns.
Held-out extraction and all final quality comparisons remain outstanding.

After a replay, audit receipt coverage and source provenance from a read-only
SQLite snapshot:

```sh
python3 scripts/memory_benchmark.py audit-extraction /path/to/sources.json /path/to/run/memory.sqlite3 /path/to/new-audit-directory
```

The command always writes `audit.json` and exits unsuccessfully for missing or
changed receipts, messages without completed extraction work, unfinished work, or
invalid evidence in currently retrievable memories. Only a successful audit
exports `memories.json`. This is a structural coverage and provenance check, not
a semantic quality score; required and forbidden policy facts still need grading.

## Project-policy development results

The frozen 12-case suite completed all 15 receipts and 21 extraction/consolidation
work items, with zero failed or retried work items. The final store has five
active memories and one superseded memory. All currently retrievable evidence
passed the exact-source audit.

Direct semantic inspection by the current Codex main agent found all 12 required
facts preserved and none of the 15 forbidden claims asserted as current memory;
12/12 cases pass. This is a single run on a small authored development suite,
not a blinded independent judge result or a general accuracy estimate. In
particular, historical evidence can quote the superseded Germany requirement
without asserting it as the current region. The retained Beacon rationale
attributes the contractual claim to the assistant and labels it unverified.

The repository contains the [per-case grades](benchmarks/memory/policy-v1/grades.json),
[actual memory snapshot](benchmarks/memory/policy-v1/memories.json),
[coverage audit](benchmarks/memory/policy-v1/audit.json),
[aggregate scores](benchmarks/memory/policy-v1/scores.json),
[usage](benchmarks/memory/policy-v1/usage.json), and
[grading provenance](benchmarks/memory/policy-v1/metadata.json). These artifacts
contain only the authored policy fixture, not third-party LoCoMo conversations.

```sh
python3 scripts/memory_benchmark.py score-policy crates/agentix-memory/tests/fixtures/quality/policy-cases.json docs/benchmarks/memory/policy-v1/grades.json /path/to/new-policy-scores.json
```

The scorer requires exactly one grade per case and one boolean per required and
forbidden fact. It aggregates supplied semantic judgments; it does not replace
them with keyword matching. Empty-output negative cases pass only when no
forbidden claim is stored, and positive cases additionally require every fact.
These policy scores do not replace full LoCoMo extraction, retrieval, answer
quality, or matched competitor evaluation, which remain incomplete.

## Querying the extracted store

Use the extracted-store adapter after a successful full-source audit:

```sh
cargo run -p agentix-memory --example extracted_retrieval_benchmark -- /path/to/run/memory.sqlite3 /path/to/questions.json http://127.0.0.1:11435 /path/to/new-query-run
```

Use `-` instead of the endpoint for FTS only. Questions contain `id`, `project`,
and `question`. The adapter takes a consistent SQLite snapshot with `VACUUM INTO`
through a read-only source connection; embedding writes affect only the snapshot.
It preserves memory IDs, revisions, rationale, conditions, tags and evidence.
Production `EmbeddingIndex` constructs document embeddings and
`SemanticRetrieval` performs both query modes. All current memories must have
current-generation vectors before queries begin. Any semantic fallback makes
the run fail after saving its actual mode for diagnosis.

`results.jsonl` retains ranked full memories, per-memory source-message IDs,
actual mode, and retrieval latency including query embedding. The
`completion.json` marker is written only after all queries finish. An empty
queue alone is insufficient to prove full input coverage; retain the separate
source audit. Timings use a 120-second benchmark semantic deadline, not the
shorter production latency budget. Queries run once in supplied order with the
production embedding cache; repeated identical queries may reuse that cache.

The first integration smoke queried five positive policy projects, each with one
active memory. FTS and real Ollama BGE-M3 hybrid both returned the expected
memory for all five, with no fallback. This checks adapter wiring only, not
ranking quality against distractors. The run used the local Rust 1.98 build;
the adapter is additionally tested with the repository-pinned Rust 1.95 toolchain.
The first FTS call included cold tokenizer initialization (262 ms); subsequent
FTS calls were below 1 ms and hybrid calls were 20–58 ms. These five observations
are not a performance distribution or evidence that hybrid is faster than FTS.

The existing `score` command also accepts extracted result files. It computes
source-evidence recall, hit rate, complete-evidence rate and reciprocal rank
across the top 5/10/20 memories. Multiple memories citing the same source cannot
inflate recall. Memory-level nDCG is omitted: source annotations do not define
an ideal ranking over memories that bundle several evidence turns. These
metrics establish evidence retrieval only, not whether the stored conclusion
contains an answer faithfully; the reader evaluation must assess that separately.

## Fixed answer-reader protocol

`scripts/memory_reader.py` runs `gpt-6-astra` with low reasoning effort, fresh
ephemeral CLI requests and no native tools. Each request contains only its
question and a ranked context prefix: at most 10 records and 24,000 UTF-8 bytes
for the serialized context array. A record that would exceed the budget and all
following records are omitted; claims are never truncated midway. The model
returns a concise answer, an abstention flag and supporting context IDs. Native
tool events, missing completed usage, and citations outside context fail the
attempt. Three attempts are allowed; all attempt artifacts are retained.

Raw retrieval passes original attributed dialogue text. Extracted-memory
retrieval passes title, conclusion, rationale, scope and conditions, excluding
original evidence quotes. Otherwise raw source quotes could conceal information
lost during extraction. Neither path passes gold answers, evidence annotations,
category labels or retrieval mode to the reader. Dates present in retrieved
text remain available, and the reader must abstain on unsupported questions.

```sh
python3 scripts/memory_reader.py /path/to/questions.json /path/to/results.jsonl /path/to/corpus.json /path/to/new-reader-run --mode fts --split development
```

Use `--mode hybrid` for the matched hybrid run, and `-` for corpus when results
contain extracted memories. The runner supports `--resume` with an exclusive
POSIX process lock and matching input/script/model/CLI fingerprints. It saves
per-attempt payloads, responses, events, errors, usage and latency. It writes a
completion marker only after every selected question has a validated response.
The development split includes all 304 questions, including 71 adversarial
questions and three with invalid source-evidence annotations; invalid evidence
affects retrieval scoring, not whether an answer should be evaluated.

This local reader is a fixed experimental protocol, not the official LoCoMo or
Mem0 reader. Semantic correctness and token F1 must be reported separately, with
judge provenance and adversarial abstention. No answer-quality score is claimed
until generation and grading cover the complete selected set.

Reader v1 completed the five-query policy integration smoke. All outputs had
valid citation IDs and no native tool activity. Semantic inspection identified
a scope-inference concern: the Chinese response expanded an internal-test
exemption into permission to use cloud embeddings. An exemption from one
restriction does not establish unconditional permission. Retain this baseline
failure for the semantic rubric; valid citations alone do not prove correctness.

Two reader-v1 runs are processing the complete 304-question development split over
raw FTS and raw hybrid retrieval respectively. Both use the same pinned script,
model, question order and context limits. These are full development answer runs,
not held-out evaluation or substitutes for the extracted-memory runs.

### Answer scoring and semantic grading

After a reader run completes, run the mode-blind judge and aggregate its labels:

```sh
python3 scripts/memory_judge.py /path/to/questions.json /path/to/reader-run /path/to/new-judge-run
python3 scripts/memory_answer_scores.py /path/to/questions.json /path/to/new-answer-scores.json --run /path/to/reader-run --split development --grades /path/to/new-judge-run/grades.json
```

The judge uses a fresh `gpt-6-astra` low-reasoning request per answer, with no
native tools. It receives the question, reference answer, adversarial flag,
candidate answer and the exact context supplied to the reader. It does not
receive the retrieval mode. Correctness requires the full requested meaning,
including dates, list items, attribution and scope. Faithfulness is assessed
against retrieved context only: a correct guess unsupported by that context
fails faithfulness. All labels include an explanation. This remains a
same-model automated judgment, not an independent human evaluation.

Judge resumption requires unchanged question, reader, answer and script
fingerprints. Inputs, responses, events, errors and usage are retained. A
completion marker and combined grades are written only after all answers have
been graded. The scorer requires exactly one answer and, when supplied, one
grade for every question/mode pair; partial sets cannot produce full scores.

Report non-adversarial semantic accuracy and faithfulness separately from
adversarial abstention, with category and conversation breakdowns. The scorer's
additional token F1 is a diagnostic unstemmed word-overlap measure. It is **not
the official LoCoMo category-aware, Porter-stemmed F1**, and must not be compared
to published official F1 results. No semantic grade is inferred from that metric.

For an additional upstream-compatible column, install `nltk==3.9.2` in an
isolated Python environment and add `--official-f1` to the scorer. This adds
`locomo_f1` for categories 1–4 and `locomo_phrase_accuracy` for category 5.
It follows the pinned [LoCoMo evaluator](https://github.com/snap-research/locomo/blob/3eb6f2c585f5e1699204e3c3bdf7adc5c28cb376/task_eval/evaluation.py):
Porter stemming, article/conjunction removal, per-reference best matching for
comma-separated category-1 items, and only the first semicolon-separated
category-3 reference. Category 5 recognizes two literal refusal phrases.
The original answer text is scored without rewriting it from the structured
abstention flag. Consequently, a valid differently worded refusal can fail this
compatibility metric, and an answer listing extra alternatives may get full
category-1 credit. Strict semantic correctness remains the primary answer metric.

The reusable upstream parity check requires `numpy==2.4.3` and the pinned clone:

```sh
LOCOMO_EVALUATOR=/path/to/locomo/task_eval/evaluation.py python3 -m unittest discover -s scripts/tests -p test_memory_answer_scores.py
```

This verifies the upstream file hash before loading only its scoring functions,
then compares all five categories on 405 synthetic input/reference combinations.
It excludes unrelated BERTScore imports and does not download model weights.
The optional dependency and parity tests are skipped in the ordinary dependency-free
suite; run the command above in the scoring environment before reporting this column.

The five-question policy reader diagnostic completed grading: four answers were
correct and faithful; the Chinese internal-test answer failed both checks for
expanding an exemption into permission. See the
[grades](benchmarks/memory/policy-reader-v1/grades.json),
[scores](benchmarks/memory/policy-reader-v1/scores.json), and
[provenance](benchmarks/memory/policy-reader-v1/metadata.json).
These references were authored after inspecting the reader outputs, so this is
a grading integration diagnostic, not a blinded quality benchmark. One earlier
diagnostic run was discarded after its Cedar reference incorrectly introduced
an onsite security officer; the corrected reference uses only the procurement
committee requirement in the source fixture. The invalid run remains retained
outside the repository and is excluded from reported results.

## Published comparison references

The following are vendor-reported LoCoMo scores checked on September 30, 2026.
They are reference points, not matched local runs or a ranking against Taskix.

| System / protocol | Reported answer score | Reported context | Source |
| --- | --- | --- | --- |
| Mem0 current platform | 92.5%, 1,425/1,540 | Mean 6,956 tokens | [Research](https://mem0.ai/research), [pinned harness](https://github.com/mem0ai/memory-benchmarks/blob/4b61c5d31b9c668a12b4f5e78064248a02c82d2b/README.md) |
| Zep multi-scope retrieval | 94.7%, 1,459/1,540 | Median 5,760 tokens | [Research](https://www.getzep.com/research/) |
| Zep auto search | 86.5% | Median 2,680 tokens | [Research](https://www.getzep.com/research/) |
| Taskix FTS / BGE-M3 hybrid | Pending complete answer generation and grading | Same local reader limits for both modes | This report |

Zep identifies its reader as GPT-5.4 with medium reasoning and its judge as
GPT-5.4. Its published category counts sum to 1,436/1,539, inconsistent with
the headline 1,459/1,540; the category distribution also differs from our pinned
dataset. Do not derive matched category comparisons from that table.

The pinned Mem0 [judge prompt](https://github.com/mem0ai/memory-benchmarks/blob/4b61c5d31b9c668a12b4f5e78064248a02c82d2b/benchmarks/locomo/prompts.py)
accepts partial list overlap and permits date differences up to 14 days and
duration differences up to 50%. Our primary rubric requires all requested facts
and correct dates. Those grades measure different acceptance criteria. A future
compatibility-grade column must be explicitly separate from strict correctness;
published percentages cannot establish superiority under our rubric. Context
budgets, model choices, dataset revisions and commercial versus OSS system
versions also need matching before attributing score differences to memory.

### Isolated lexical experiments

The raw retrieval example accepts an optional final argument: `baseline`
(default), `porter`, `baseline-stop`, or `porter-stop`. Porter uses SQLite's
English stemming tokenizer in a fresh temporary benchmark database; `-stop`
filters common English function words from the lexical query only. Query
embeddings retain the original question. These switches do not alter production
indexes or migrate existing stores. Run experiments on development questions
with the same complete corpus and vector cache before freezing a held-out run.

Development-only evidence Recall@10 (230 answerable questions with valid
evidence annotations; all 304 development questions queried in both modes):

| Lexical variant | FTS | BGE-M3 hybrid |
| --- | ---: | ---: |
| baseline | 58.38% | 63.25% |
| porter | 62.04% | 66.62% |
| baseline-stop | 60.23% | 63.75% |
| porter-stop | 64.98% | 66.39% |

All runs index the same 5,882 attributed source turns and reuse the same BGE-M3
vector cache. See [aggregate scores](benchmarks/memory/lexical-ablation-v1/scores.json)
and [artifact fingerprints](benchmarks/memory/lexical-ablation-v1/manifest.json).
Porter improves both modes. Removing function words further improves FTS, but
slightly reduces hybrid Recall@10 and complete-evidence coverage versus Porter
alone. The combined variant is therefore not uniformly best. These development
results nominate candidates for answer evaluation; they do not establish held-out
quality or justify a production tokenizer migration by themselves.

### Matched Mem0 OSS adapter

The local comparator uses Mem0 OSS 2.2.1 at commit
`94c3fe9f238f3dbf29c9ce98643bd71eb13077cd`, installed in an isolated environment.
`scripts/memory_mem0_codex.py` implements its synchronous model-provider interface
using the same `gpt-6-astra` low-reasoning Codex execution settings. It preserves
Mem0's system prompt and conversation messages, records usage and raw responses,
and rejects native tool activity or unsupported provider options. Mem0's fixed
provider-name validation requires registering this adapter under `openai` in the
benchmark process's factory; it does not call the OpenAI SDK or modify Mem0 source.
The extraction, update and retrieval logic remains upstream code. This local
adapter is distinct from the vendor's hosted platform and published scores.

The optional live wiring test uses local Qdrant, Ollama BGE-M3 (1,024 dimensions),
an isolated history database, and a synthetic procurement decision. It checks
extraction, retrieval and project isolation; it is not a quality benchmark:

```sh
MEM0_TELEMETRY=false MEM0_DIR=/path/to/isolated-runtime \
TASKIX_BENCH_MEM0_CALLS=/path/to/calls \
TASKIX_BENCH_MEM0_SMOKE=/path/to/new-smoke-directory \
python3 -m unittest discover -s scripts/tests -p test_memory_mem0_codex.py
```

Install the pinned Mem0 source and `ollama` in that environment first, and run
BGE-M3 at `http://127.0.0.1:11435`. Use a new smoke directory for each invocation;
retain configuration, call receipts and results. The full LoCoMo comparator
requires source coverage and immutable replay manifests before reporting scores.

The first live smoke passed extraction, BGE-M3 retrieval and project isolation,
but logged missing fastembed and spaCy dependencies. Its keyword search was
disabled. Treat it strictly as model/storage wiring evidence; it must not become
the scored Mem0 comparator. Install and verify the lexical/NLP dependencies,
then repeat the smoke with a fresh store before full replay.

The repeated smoke with fastembed 0.8.1, spaCy 3.8.16 and en_core_web_sm 3.8.0
passed all three tests, including a nonempty BM25 result. Local Qdrant warns that
payload indexes are ineffective in embedded mode; this affects performance, not
the tested project filter. This remains wiring validation, not a LoCoMo score.

`scripts/memory_mem0_replay.py` runs complete per-project source replay and native
Mem0 hybrid retrieval. Each source is applied to a private copy of the last
closed Qdrant/history store. Only a successful, closed attempt advances the atomic
checkpoint; failed attempts retain their call logs and cannot enter later
results. `--resume` checks ordered source, question, code, dependency, model and
embedding fingerprints. A POSIX lock excludes concurrent writers to one run.
Successful old store copies are removed while receipts and model calls remain.
The checkpoint protocol covers process interruption, not power-loss durability.

```sh
MEM0_TELEMETRY=false MEM0_DIR=/path/to/isolated-runtime \
python3 scripts/memory_mem0_replay.py corpus.json questions.json /path/to/new-run \
  --project conv-26 --mem0-source /path/to/pinned-mem0
```

Use separate run directories for each project. The runner requires the pinned
BGE-M3 digest and complete NLP/BM25 initialization, rejects malformed extraction
responses and logged extraction degradation, and uses upstream inference rather
than direct fact insertion. Original dates and speaker attribution remain in the
source text. OSS rejects the hosted platform's historical `timestamp` argument,
so no such argument is supplied. Query export contains only stored memory text,
with a project check and no raw-source fallback. Extraction and query completion
are recorded separately; combine only complete project exports before using the
same answer reader and rubric. Per-source timings include store reopen/close and
checkpoint overhead and are not native Mem0 throughput measurements.

### Development extraction diagnostic

A partial source-level inspection found that `conv-26/D1:3`, an explicitly
dated historical experience, completed extraction with zero candidates. Other
inspected memories retained career intentions and long-term preferences, including
compatible merges. See the [diagnostic receipt](benchmarks/memory/development-retention-audit-v1.json).
This identifies a retention-policy hypothesis: the extractor may treat useful
historical facts as transient updates. It does not prove final recall or answer
accuracy, and later sources may reintroduce the same information. Preserve the
frozen baseline and test any retention change on development data together with
project-policy exclusions before evaluating held-out conversations.

The opt-in regression below checks the original development failure against a
real replay store. It verifies direct evidence, speaker/date retention and no
expiry for a dated historical assertion. It failed on the frozen baseline because
no active memory cited the source. The candidate extraction/consolidation prompts
clarify that supported experiences with lasting significance are historical
knowledge; they still exclude task progress, tool errors and repository facts.
This candidate requires live replay and exclusion-policy validation before any
claim of improved quality.

```sh
TASKIX_BENCH_RETENTION_DB=/path/to/replay/memory.sqlite3 \
python3 -m unittest discover -s scripts/tests -p test_memory_retention.py
```
