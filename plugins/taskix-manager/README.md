# Taskix Manager implementation

## Bundled entrypoints

| Host | Plugin configuration | Lifecycle entrypoint |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json` | Explicit `hooks/hooks.json` and `hooks/codex.json` |
| Claude Code | `.claude-plugin/plugin.json` | Default `hooks/hooks.json` plus manifest `hooks/claude.json` |
| Pi | `package.json` → `pi.extensions` | `extensions/pi.ts` |
| OMP | `package.json` → `omp.extensions` | `extensions/omp.ts` |
| OpenCode 2.0.32+ | Taskix package default export | `opencode/server.mjs` |

Codex and Claude share the common lifecycle hooks. Codex explicitly loads the shared file and a separate Interrupt hook; its manifest replaces default discovery, so each hook loads once. Claude merges default discovery of the shared file with its manifest-selected `hooks/claude.json`, which only adds PostToolUseFailure. Do not repeat the shared file in Claude’s manifest. Pi and OMP each select exactly one extension, so neither loads the other host's entrypoint. Pi declares the shared `skills/` directory in its manifest. OMP installs the marketplace plugin and discovers its shared `skills/` directory. The npm `files` list includes host manifests, hooks, extensions, OpenCode entrypoints, runtime, skills, Taskix Sync and its board styles, and this technical reference.

## Runtime boundaries

| Module | Responsibility |
| --- | --- |
| `jev.mjs` | Provider requests, confidence checks and semantic routing advice |
| `routing-context.mjs` | Pure, bounded rendering of fallback hints; no I/O or task writes |
| `runtime.mjs` | Shared Codex/Claude/Pi/OMP/OpenCode orchestration, deadlines, receipts and heartbeats |
| `taskix-cli.mjs` | Shell-free CLI execution, host identity arguments and error handling |
| `discussion.mjs` | Discussion selection and its standalone command-line entrypoint |

The runtime re-exports `buildArgs` and `runTaskix` for existing callers. The standalone
discussion helper loads the CLI adapter directly, avoiding a runtime/discussion
import cycle. Its entrypoint recognizes canonical and symlinked installation paths.

Fallback instructions refer to the workflow skill instead of repeating its full
policy. Identity hints have priority over body excerpts. The renderer considers at
most 32 Job and 32 Issue candidates, serializes each fragment once, and accounts for
JSON escaping and separators before appending it. Counts and `candidates_complete`
make omitted evidence explicit. A discussion-only prompt needs no lifecycle write;
Jev confidence thresholds and task ownership rules are unchanged.

## Validation

The Obsidian status bridge sends revision-guarded human completion commands as
`user:obsidian`. Completing a Job finishes its unfinished Tasks and revokes their
leases in the same transaction, while preserving terminal outcomes. Individual
unfinished Tasks can also be completed without an agent Plan or lease. Agent
entrypoints retain their normal ownership and execution guards.

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

## OpenCode server boundary

OpenCode 2.0.32+ loads the Taskix Manager package's default export separately
from the Bridge package's `./tui` export. Both use native `{ id, setup(context) }`
plugins. Taskix registers prompt/context hooks, a plain JSON Schema tool and an
abortable native event subscription. The tool is registered outside Code Mode
so each invocation keeps its own native call ID for idempotency.

Prompt preparation only stages context. A queued prompt becomes active on native
inbox delivery; admitting a later prompt does not replace the running turn's
Taskix context. Discussion provenance uses the delivered native inbox/user ID.
Minimal active ownership/context state is saved in native plugin storage for
restart and compacted-history recovery. Cleanup aborts event consumption and
stops lease timers. Native execution terminal events settle promoted turns;
individual model/tool steps and assistant step errors do not. Cleanup preserves
active source/context for native continuation rather than recording a completed
reply. Failed activation can retry with the same native source ID.

Dispatch allows sixteen outstanding event jobs globally and four per session,
with native-stream backpressure and per-session ordering. Active work, leases and
dispatcher work pin wrappers against the inactive-session LRU limit.

Use `make plugin HOSTS=opencode SOURCE=main` for native GitHub package references,
or `SOURCE=local` to copy the current checkout and register local `file://` paths.
Both modes configure the server and TUI packages without third-party runtime npm
dependencies or Zod. Switching modes replaces previous installer-owned references;
remote installation removes the old local copy only after both packages install.
Use `opencode plugin update` for remote updates and `make remove-plugin HOSTS=opencode`
to remove either installation mode. Earlier 2.0 patches and
OpenCode 1.x are unsupported; 2.0.32 is the validated baseline.
See the [OpenCode guide](https://github.com/tenfyzhong/agentix/wiki/OpenCode) for installation.
