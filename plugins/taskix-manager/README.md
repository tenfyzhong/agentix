# Taskix Manager

The user guide has moved to the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki/Taskix-Manager). Start with the [Wiki home](https://github.com/tenfyzhong/agentix/wiki/Home) for installation, configuration, and everyday use.

The implementation and development notes below remain in the repository.

## Bundled entrypoints

| Host | Plugin configuration | Lifecycle entrypoint |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json` | Explicit `hooks/hooks.json` and `hooks/codex.json` |
| Claude Code | `.claude-plugin/plugin.json` | Default `hooks/hooks.json` plus manifest `hooks/claude.json` |
| Pi | `package.json` → `pi.extensions` | `extensions/pi.ts` |
| OMP | `package.json` → `omp.extensions` | `extensions/omp.ts` |

Codex and Claude share the common lifecycle hooks. Codex explicitly loads the shared file and a separate Interrupt hook; its manifest replaces default discovery, so each hook loads once. Claude merges default discovery of the shared file with its manifest-selected `hooks/claude.json`, which only adds PostToolUseFailure. Do not repeat the shared file in Claude’s manifest. Pi and OMP each select exactly one extension, so neither loads the other host's entrypoint. Pi declares the shared `skills/` directory in its manifest. OMP installs the marketplace plugin and discovers its shared `skills/` directory. The npm `files` list includes all four host manifests, hooks, extensions, runtime, skills, TaskNotes settings and setup guide, and this guide.

## Runtime boundaries

| Module | Responsibility |
| --- | --- |
| `jev.mjs` | Provider requests, confidence checks and semantic routing advice |
| `routing-context.mjs` | Pure, bounded rendering of fallback hints; no I/O or task writes |
| `runtime.mjs` | Shared Codex/Claude/Pi/OMP orchestration, deadlines, receipts and heartbeats |
| `taskix-cli.mjs` | Shell-free CLI execution, host identity arguments and error handling |
| `discussion.mjs` | Discussion selection and its standalone command-line entrypoint |

The runtime re-exports `buildArgs` and `runTaskix` for existing callers. The standalone
discussion helper loads the CLI adapter directly, avoiding a runtime/discussion
import cycle. Its entrypoint recognizes canonical and symlinked installation paths.

Fallback instructions refer to the workflow skill instead of repeating its full
policy. Identity hints have priority over body excerpts. The renderer considers at
most 32 Job and 32 Inbox entries, serializes each fragment once, and accounts for
JSON escaping and separators before appending it. Counts and `candidates_complete`
make omitted evidence explicit. A discussion-only prompt needs no lifecycle write;
Jev confidence thresholds and task ownership rules are unchanged.

## Validation

The remote-package installation test uses an empty npm cache and offline
installation with development dependencies omitted. It verifies Pi checkout
installs and OMP package-consumer installs, including the canonical plugin skill
directory and its contained reference links. The cold-install tests additionally
copy the plugin directory and unpack its npm tarball into isolated directories
without `node_modules`, then execute the hook, check its error handler and
environment activation, load both command helpers, and register Pi/OMP adapters.

## Optional lifecycle assessment

With the existing `TASKIX_JEV_ENABLED` configuration, prompt routing also classifies review policy and explicit user acceptance, rejection, Job cancellation and Task-operation intent. A proposal accepted for implementation remains work; delivery acceptance is distinct. Existing required review is preserved locally; code supplements upgrade a none policy. Disabling Jev retains the original main-Agent workflow.

`lifecycle.mjs` supplies a shared read-only checkpoint for Codex/Claude shell calls and Pi/OMP's `taskix` structured tool. Recovery can select a Task from the existing Job candidates, assess whether waiting reasons have been resolved, or classify explicit retry, reopen, cancellation and handoff. Outcome assessment distinguishes continuing work, waiting for the user, external blockage, final failure and readiness for independent acceptance verification. Reassessment of review policy uses the same pipeline when scope changes. Before the final Task finishes an ACTIVE Job, the `completion` checkpoint asks Jev to select `pending_review` or `completed` from the entire delivered scope. Execute its guarded atomic completion command after verification, using the final Task ID and current lease. Policy and status are committed together. Upgrade the CLI and plugin together. This can replace an earlier required default for a wholly non-code Job; it cannot serve as approval of already pending work. Disabled or uncertain results retain the saved policy.

All assessments reuse the existing filtered Job/Task facts and visible conversation excerpts, 30,000-byte request ceiling, eight-second deadline, score gates and current-Job revision check. No raw tool results, source files, lease credentials or reasoning are collected. Explicit terminal Task recovery reads only that named Task and projects the same title/status/reason fields within existing bounds; normal prompt candidate discovery is unchanged. Shared instructions are sent once rather than repeated for every Job. Each assessment makes one provider request; disabled configuration makes no CLI or HTTP calls.

Results contain guarded command arguments, never execute them, and defer to the main Agent on uncertainty, failure, oversize or stale evidence. Actual CLI lease, dependency, Plan and transition guards remain authoritative. `ready` is not `done` and never authorizes self-approval. Prompt metrics include the work-scope question when it can change the selected policy; lifecycle checkpoints have separate request-kind statistics. See [command examples](skills/taskix-manager/references/commands.md#optional-lifecycle-classification).
