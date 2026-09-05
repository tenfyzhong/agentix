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

export async function runHook(event, runner = runTaskcli) {
    if (!event.session_id) throw new Error("Hook requires session_id");
    const operation =
        event.hook_event_name === "SessionStart"
            ? "session-start"
            : event.hook_event_name === "SessionEnd"
              ? "session-end"
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
    let timer;
    let activeSession;
    let refreshing = false;
    // Retries of one host call must retain the original authorization input,
    // even when the first attempt committed and released the current lease.
    const requestTokens = new Map();
    const optionsFor = (ctx) => ({
        cwd: ctx.cwd,
        session: ctx.sessionManager.getSessionId(),
        executor: `agent:${host}:${ctx.sessionManager.getSessionId()}`,
    });
    const clear = () => {
        if (timer !== undefined) timers.clearInterval(timer);
        timer = undefined;
    };
    api.on("session_start", async (_event, ctx) => {
        clear();
        const options = optionsFor(ctx);
        if (activeSession && activeSession !== options.session)
            await runner(["hook", "session-end"], {
                ...options,
                session: activeSession,
            });
        activeSession = options.session;
        await runner(["hook", "session-start"], options);
        timer = timers.setInterval(async () => {
            if (refreshing) return;
            refreshing = true;
            try {
                await runner(["hook", "heartbeat"], options);
            } catch (error) {
                ctx.ui?.notify?.(
                    `Task heartbeat failed: ${error.message}`,
                    "warning",
                );
            } finally {
                refreshing = false;
            }
        }, 60000);
        timer?.unref?.();
    });
    api.on("session_shutdown", async (_event, ctx) => {
        clear();
        await runner(["hook", "session-end"], optionsFor(ctx));
        activeSession = undefined;
    });
    api.on("before_agent_start", async (_event, ctx) => {
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
            if (
                params.args[0] === "task" &&
                params.args[1] !== "claim" &&
                params.args[2] === context.result.task_id
            )
                options.token = context.result.lease?.token;
            if (
                params.args[0] === "plan" &&
                params.args[2] === context.result.task_id
            )
                options.token = context.result.lease?.token;
            const writes = new Set([
                "add",
                "create",
                "update",
                "register",
                "revise",
                "claim",
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
                "depend",
                "undepend",
            ]);
            if (
                writes.has(params.args[1]) &&
                !params.args.some((a) => a.startsWith("--idempotency-key"))
            )
                options.idempotencyKey = `${host}:${options.session}:${toolCallId}`;
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
                if (key) {
                    const scopedKey = JSON.stringify([options.session, key]);
                    if (requestTokens.has(scopedKey))
                        options.token = requestTokens.get(scopedKey);
                    else {
                        requestTokens.set(scopedKey, options.token);
                        if (requestTokens.size > 512)
                            requestTokens.delete(
                                requestTokens.keys().next().value,
                            );
                    }
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
