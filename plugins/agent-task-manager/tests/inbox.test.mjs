import assert from "node:assert/strict";
import { test } from "node:test";
import { registerExtension, runHook } from "../runtime.mjs";

const claimed = { claimed: true, entry: { id: "inbox_one", job_id: "job_one", revision: 2 }, job: { id: "job_one" } };

test("Stop hooks continue a claimed Inbox Job and respect empty queues and plan mode", async () => {
    for (const host of ["codex", "claude"]) {
        const calls = [];
        let next = claimed;
        const runner = async (args, options) => {
            calls.push({ args, options });
            return { result: args[1] === "stop" ? next : {} };
        };
        const event = { hook_event_name: "Stop", session_id: "session", cwd: "/work", ...(host === "codex" ? { turn_id: "turn" } : {}) };
        const output = await runHook(event, runner);
        assert.equal(output.decision, "block");
        assert.match(output.reason, /job_one/);
        assert.equal(calls.at(-1).options.executor, `agent:${host}`);
        next = { claimed: false, reason: "active_jobs" };
        assert.deepEqual(await runHook({ ...event, stop_hook_active: true }, runner), {});
        next = claimed;
        const before = calls.filter(c => c.args[1] === "stop").length;
        assert.deepEqual(await runHook({ ...event, permission_mode: "plan" }, runner), {});
        assert.equal(calls.filter(c => c.args[1] === "stop").length, before);
    }
});

test("tool hooks surface human cancellation without restarting an interrupted turn", async () => {
    const runner = async args => ({ result: args[0] === "context" ? { inbox_cancellations: [{ id: "inbox_one", job_id: "job_one" }] } : {} });
    const event = { session_id: "session", cwd: "/work" };
    const result = await runHook({ ...event, hook_event_name: "PreToolUse" }, runner);
    assert.match(result.hookSpecificOutput.additionalContext, /cancelled/i);
    assert.match(result.hookSpecificOutput.additionalContext, /job_one/);
    assert.deepEqual(await runHook({ ...event, hook_event_name: "Interrupt" }, runner), {});
});

for (const host of ["pi", "omp"]) {
    test(`${host} releases intake if a new turn starts while the claim is pending`, async () => {
        const handlers = new Map(), messages = [], calls = [];
        let resolveClaim, started;
        const waiting = new Promise(resolve => { started = resolve; });
        registerExtension({ on: (name, fn) => handlers.set(name, fn), registerTool() {}, sendMessage: message => messages.push(message) }, host,
            async args => {
                calls.push(args);
                if (args[1] === "stop") {
                    started();
                    return new Promise(resolve => { resolveClaim = resolve; });
                }
                return { result: {} };
            }, { setInterval: () => 1, clearInterval() {} });
        const ctx = { cwd: "/work", isIdle: () => true, sessionManager: { getSessionId: () => "session" } };
        await handlers.get("session_start")({}, ctx);
        await handlers.get("agent_start")({}, ctx);
        const end = handlers.get("agent_end")({ messages: [{ role: "assistant", stopReason: "stop" }] }, ctx);
        const pending = host === "pi" ? end.then(() => handlers.get("agent_settled")({}, ctx)) : end;
        await waiting;
        await handlers.get("agent_start")({}, ctx);
        resolveClaim({ result: { ...claimed, entry: { ...claimed.entry, lease: { token: "new_lease" } } } });
        await pending;
        assert.equal(messages.length, 0);
        assert.ok(calls.some(args => args[0] === "inbox" && args[1] === "release"));
        await handlers.get("session_shutdown")({}, ctx);
    });

    test(`${host} a new Inbox lease can continue a previously delivered entry`, async () => {
        const handlers = new Map(), messages = [];
        let revision = 2;
        registerExtension({ on: (name, fn) => handlers.set(name, fn), registerTool() {}, sendMessage: message => messages.push(message) }, host,
            async args => ({ result: args[1] === "stop" ? { ...claimed, entry: { ...claimed.entry, revision } } : {} }),
            { setInterval: () => 1, clearInterval() {} });
        const ctx = { cwd: "/work", isIdle: () => true, sessionManager: { getSessionId: () => "session" } };
        await handlers.get("session_start")({}, ctx);
        for (revision of [2, 4]) {
            await handlers.get("agent_start")({}, ctx);
            await handlers.get("agent_end")({ messages: [{ role: "assistant", stopReason: "stop" }] }, ctx);
            if (host === "pi") await handlers.get("agent_settled")({}, ctx);
        }
        assert.equal(messages.length, 2);
        await handlers.get("session_shutdown")({}, ctx);
    });
    test(`${host} only continues after a successful terminal settle and deduplicates delivery`, async () => {
        const handlers = new Map(), messages = [], calls = [];
        const api = { on: (name, fn) => handlers.set(name, fn), registerTool() {}, sendMessage: (...args) => messages.push(args) };
        const ctx = { cwd: "/work", isIdle: () => true, hasPendingMessages: () => false, sessionManager: { getSessionId: () => "session" } };
        registerExtension(api, host, async args => {
            calls.push(args);
            return { result: args[1] === "stop" ? claimed : {} };
        }, { setInterval: () => 1, clearInterval() {} });
        const emit = (name, event = {}) => handlers.get(name)(event, ctx);
        await emit("session_start");
        await emit("agent_start");
        await emit("agent_end", { messages: [{ role: "assistant", stopReason: "stop" }] });
        if (host === "pi") {
            assert.equal(messages.length, 0);
            await emit("agent_settled");
        }
        assert.equal(messages.length, 1);
        assert.equal(messages[0][1].triggerTurn, true);
        assert.equal(messages[0][1].deliverAs, "followUp");
        assert.match(messages[0][0].content, /job_one/);
        await emit(host === "pi" ? "agent_settled" : "agent_end", { messages: [{ role: "assistant", stopReason: "stop" }] });
        assert.equal(messages.length, 1);
        await emit("agent_start");
        const before = calls.filter(c => c[1] === "stop").length;
        await emit("agent_end", { messages: [{ role: "assistant", stopReason: "aborted" }], willContinue: true });
        if (host === "pi") await emit("agent_settled");
        assert.equal(calls.filter(c => c[1] === "stop").length, before);
        await emit("agent_end", { messages: [{ role: "assistant", stopReason: "aborted" }] });
        if (host === "pi") await emit("agent_settled");
        assert.equal(messages.length, 1);
        await emit("session_shutdown");
    });

    test(`${host} Inbox release receives its own lease and preserves retry credentials`, async () => {
        const registered = [], calls = [];
        let token = "inbox_lease";
        registerExtension({ on() {}, registerTool: tool => registered.push(tool) }, host, async (args, options) => {
            if (args[0] === "context") return { result: { inbox: { id: "inbox_one", lease: { token } }, lease: { token: "task_lease" } } };
            calls.push(options);
            token = undefined;
            return { result: {} };
        });
        const ctx = { cwd: "/work", sessionManager: { getSessionId: () => "session" } };
        for (let i = 0; i < 2; i++) await registered[0].execute("call-one", { args: ["inbox", "release", "inbox_one"] }, undefined, undefined, ctx);
        assert.ok(calls.every(c => c.token === "inbox_lease"));
        assert.equal(calls[0].idempotencyKey, calls[1].idempotencyKey);
    });
}
