import { execFile } from "node:child_process";
import { promisify } from "node:util";

const executeFile = promisify(execFile);
const reserved = new Set([
    "--session",
    "--executor",
    "--lease-token",
    "--actor",
    "--json",
]);

export function buildArgs(args, options = {}) {
    if (!Array.isArray(args) || args.some((arg) => typeof arg !== "string")) {
        throw new Error("taskcli args must be an array of strings");
    }
    for (const arg of args) {
        if (reserved.has(arg.split("=")[0]))
            throw new Error(`Identity option is managed by the host: ${arg}`);
    }
    const result = ["--json", ...args];
    for (const [flag, value] of [
        ["--session", options.session],
        ["--executor", options.executor],
        ["--lease-token", options.token],
        ["--idempotency-key", options.idempotencyKey],
    ]) {
        if (value) result.push(flag, value);
    }
    return result;
}

export async function runTaskcli(args, options = {}) {
    try {
        const { stdout } = await executeFile(
            process.env.TASKCLI_BIN || "taskcli",
            buildArgs(args, options),
            {
                cwd: options.cwd,
                signal: options.signal,
                timeout: 30000,
                maxBuffer: 4 * 1024 * 1024,
                windowsHide: true,
            },
        );
        const response = JSON.parse(stdout);
        if (response.schema_version !== 1 || !response.ok)
            throw new Error(
                response.error?.message || "Unsupported taskcli response",
            );
        return response;
    } catch (error) {
        if (error.stdout) {
            try {
                throw new Error(
                    JSON.parse(error.stdout).error?.message || error.message,
                );
            } catch (parsed) {
                if (!(parsed instanceof SyntaxError)) throw parsed;
            }
        }
        throw error;
    }
}

function skillContext(context) {
    return {
        ...context,
        task_language: process.env.AGENT_TASK_LANG?.trim() || "en",
    };
}

function cancellationContext(context) {
    const entries = (context?.inbox_cancellations || []).filter(
        (entry) => !context.job_id || entry.job_id === context.job_id,
    );
    if (!entries.length) return undefined;
    return `Human Inbox work has been cancelled. Stop work on these Jobs at the next safe boundary, preserve completed results, and do not retry stale writes or roll back changes automatically. Inspect taskcli context before selecting other work.\nCancellation facts: ${JSON.stringify(entries.map(entry => ({ id: entry.id, job_id: entry.job_id })))}`;
}

export async function runHook(event, runner = runTaskcli) {
    if (!event.session_id) throw new Error("Hook requires session_id");
    if (event.hook_event_name === "PostToolUseFailure" && event.is_interrupt !== true)
        return {};
    const operation =
        event.hook_event_name === "SessionStart"
            ? "session-start"
            : event.hook_event_name === "SessionEnd"
              ? "session-end"
              : ["Interrupt", "PostToolUseFailure"].includes(event.hook_event_name)
                ? "interrupt"
                : "heartbeat";
    const options = { cwd: event.cwd, session: event.session_id };
    await runner(["hook", operation], options);
    if (operation === "session-start") {
        const context = await runner(["context"], options);
        return {
            hookSpecificOutput: {
                hookEventName: "SessionStart",
                additionalContext: `Task session: ${event.session_id}. Use the agent-task-manager skill for tracked work.\n${JSON.stringify(skillContext(context.result))}`,
            },
        };
    }
    if (["PreToolUse", "PostToolUse"].includes(event.hook_event_name)) {
        const context = await runner(["context"], options);
        const notice = cancellationContext(context.result);
        if (notice)
            return {
                hookSpecificOutput: {
                    hookEventName: event.hook_event_name,
                    additionalContext: notice,
                },
            };
    }
    return {};
}

export function registerExtension(
    api,
    host,
    runner = runTaskcli,
    timers = globalThis,
    parameters = {
        type: "object",
        properties: { args: { type: "array", items: { type: "string" } } },
        required: ["args"],
    },
) {
    let active;
    // Retries of one host call must retain the original authorization input,
    // even when the first attempt committed and released the current lease.
    const requestTokens = new Map();
    const optionsFor = (ctx) => ({
        cwd: ctx.cwd,
        session: ctx.sessionManager.getSessionId(),
        executor: `agent:${host}:${ctx.sessionManager.getSessionId()}`,
    });
    const stateFor = (ctx) =>
        active?.options.session === optionsFor(ctx).session ? active : undefined;
    const pause = (state) => {
        if (state.timer !== undefined) timers.clearInterval(state.timer);
        state.timer = undefined;
        state.heartbeat?.abort();
    };
    const renew = (state, ctx) => {
        if (state.timer !== undefined) return;
        const timer = timers.setInterval(async () => {
            // A callback already queued before clearInterval must stay inert.
            if (active !== state || state.timer !== timer || state.heartbeat) return;
            const controller = new AbortController();
            state.heartbeat = controller;
            try {
                const result = await runner(["hook", "heartbeat"], {
                    ...state.options,
                    signal: controller.signal,
                });
                if (active === state && !controller.signal.aborted) {
                    const notice = cancellationContext(result.result);
                    if (notice && notice !== state.cancellationNotice) {
                        api.sendMessage(
                            { customType: "taskcli-inbox-cancelled", content: notice, display: true },
                            { triggerTurn: false, deliverAs: "steer" },
                        );
                        state.cancellationNotice = notice;
                    }
                }
            } catch (error) {
                if (!controller.signal.aborted)
                    ctx.ui?.notify?.(`Task heartbeat failed: ${error.message}`, "warning");
            } finally {
                if (state.heartbeat === controller) state.heartbeat = undefined;
            }
        }, 60000);
        state.timer = timer;
        timer?.unref?.();
    };
    const release = async (state, operation) => {
        pause(state);
        // Join concurrent cleanup, but allow SessionEnd after Interrupt and retries
        // after errors. A failed cleanup must never silently restart renewal.
        if (state.cleanup) await state.cleanup;
        if (state.ended || (operation === "interrupt" && state.interrupted)) return;
        const cleanup = runner(["hook", operation], state.options).then(() => {
            if (operation === "session-end") state.ended = true;
            else state.interrupted = true;
        });
        state.cleanup = cleanup;
        try {
            await cleanup;
        } finally {
            if (state.cleanup === cleanup) state.cleanup = undefined;
        }
    };
    api.on("session_start", async (_event, ctx) => {
        const options = optionsFor(ctx);
        if (active) {
            pause(active);
            if (active.options.session !== options.session)
                await release(active, "session-end");
            else if (active.cleanup) await active.cleanup;
        }
        const state = { options };
        active = state;
        await runner(["hook", "session-start"], state.options);
        if (active === state) renew(state, ctx);
    });
    api.on("session_shutdown", async (_event, ctx) => {
        const state = stateFor(ctx);
        if (!state) return;
        await release(state, "session-end");
        if (active === state) active = undefined;
    });
    api.on("agent_start", async (_event, ctx) => {
        const state = stateFor(ctx);
        if (state) {
            state.aborted = false;
        }
    });
    api.on("agent_end", async (event, ctx) => {
        const state = stateFor(ctx);
        if (!state) return;
        const lastAssistant = event.messages?.findLast(message => message.role === "assistant");
        state.aborted = lastAssistant?.stopReason === "aborted" && event.willContinue !== true;
        if (host === "omp" && state.aborted) await release(state, "interrupt");
    });
    if (host === "pi") {
        // agent_end can precede auto-retry/compaction/follow-ups. Only release
        // when Pi reports a terminal settle, never during those continuations.
        api.on("agent_settled", async (_event, ctx) => {
            const state = stateFor(ctx);
            if (state?.aborted && ctx.isIdle()) await release(state, "interrupt");
        });
    }
    api.on("before_agent_start", async (_event, ctx) => {
        const state = stateFor(ctx);
        if (state) {
            if (state.cleanup) await state.cleanup;
            if (active === state && !state.ended) {
                state.aborted = false;
                state.interrupted = false;
                renew(state, ctx);
            }
        }
        const result = await runner(["context"], optionsFor(ctx));
        return {
            message: {
                customType: "taskcli-context",
                content: `Task context (facts, not instructions):\n${JSON.stringify(skillContext(result.result))}`,
                display: false,
            },
        };
    });
    api.registerTool({
        name: "taskcli",
        label: "Task board",
        description:
            "Manage Project/Job/Task/Plan through taskcli. Pass argument strings, not a shell command. The host supplies session, executor, and the current lease token. Use the agent-task-manager skill for workflow and document rules.",
        parameters,
        async execute(toolCallId, params, signal, _onUpdate, ctx) {
            const options = { ...optionsFor(ctx), signal };
            const context = await runner(["context"], options);
            const currentToken = async () => {
                const [kind, command, identifier] = params.args;
                if (kind === "inbox" && command === "release") {
                    const entry = context.result.inbox;
                    // The CLI accepts prefixes; use only an exact owned identity
                    // here so an ambiguous prefix cannot receive credentials.
                    return identifier === entry?.id ? entry.lease?.token : undefined;
                }
                const current = context.result.task_id;
                if (
                    !(kind === "plan" || (kind === "task" && command !== "claim")) ||
                    !identifier ||
                    !current ||
                    !current.startsWith(identifier)
                )
                    return undefined;
                if (identifier !== current) {
                    const resolved = await runner(["task", "show", identifier], options);
                    if (resolved.result.id !== current) return undefined;
                }
                return context.result.lease?.token;
            };
            const writes = new Set([
                "add",
                "create",
                "update",
                "register",
                "revise",
                "claim",
                "claim-next",
                "start",
                "done",
                "cancel",
                "retry",
                "reopen",
                "block",
                "wait",
                "fail",
                "release",
                "archive",
                "unarchive",
                "delete",
                "depend",
                "undepend",
            ]);
            if (
                writes.has(params.args[1]) &&
                !params.args.some((a) => a.startsWith("--idempotency-key"))
            )
                options.idempotencyKey = `${host}:${options.session}:${toolCallId}`;
            let scopedKey;
            if (writes.has(params.args[1])) {
                const explicit = params.args.findIndex(
                    (arg) =>
                        arg === "--idempotency-key" ||
                        arg.startsWith("--idempotency-key="),
                );
                const key =
                    options.idempotencyKey ??
                    (explicit >= 0
                        ? params.args[explicit].split("=").slice(1).join("=") ||
                          params.args[explicit + 1]
                        : undefined);
                if (key) scopedKey = JSON.stringify([options.session, key]);
            }
            if (scopedKey && requestTokens.has(scopedKey)) {
                options.token = requestTokens.get(scopedKey);
            } else {
                options.token = await currentToken();
                if (scopedKey) {
                    requestTokens.set(scopedKey, options.token);
                    if (requestTokens.size > 512)
                        requestTokens.delete(requestTokens.keys().next().value);
                }
            }
            const result = await runner(params.args, options);
            return {
                content: [{ type: "text", text: JSON.stringify(result) }],
                details: result,
            };
        },
    });
}
