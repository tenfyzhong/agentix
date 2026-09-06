import assert from "node:assert/strict";
import { test } from "node:test";
import { registerExtension, runHook } from "../runtime.mjs";

const claimed = { claimed: true, entry: { id: "inbox_one", job_id: "job_one", revision: 2 }, job: { id: "job_one" } };

test("Stop hooks leave queued Inbox work for an explicit user request", async () => {
    for (const host of ["codex", "claude"]) {
        const calls = [];
        const runner = async (args, options) => {
            calls.push({ args, options });
            return { result: args[1] === "stop" ? claimed : {} };
        };
        const event = { hook_event_name: "Stop", session_id: "session", cwd: "/work", ...(host === "codex" ? { turn_id: "turn" } : {}) };
        for (const extra of [{}, { stop_hook_active: true }, { permission_mode: "plan" }]) {
            assert.deepEqual(await runHook({ ...event, ...extra }, runner), {});
        }
        assert.deepEqual(calls.map(call => call.args), Array(3).fill(["hook", "heartbeat"]));
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
    test(`${host} completion, idle events and heartbeats never take queued Inbox work`, async () => {
        const handlers = new Map(), messages = [], calls = [];
        let heartbeat;
        const api = { on: (name, fn) => handlers.set(name, fn), registerTool() {}, sendMessage: (...args) => messages.push(args) };
        const ctx = { cwd: "/work", isIdle: () => true, hasPendingMessages: () => false, sessionManager: { getSessionId: () => "session" } };
        registerExtension(api, host, async args => {
            calls.push(args);
            return { result: args[1] === "stop" ? claimed : {} };
        }, { setInterval: fn => { heartbeat = fn; return 1; }, clearInterval() {} });
        const emit = (name, event = {}) => handlers.get(name)(event, ctx);
        await emit("session_start");
        for (const stopReason of ["stop", "stop", "error", "aborted"]) {
            await emit("before_agent_start");
            await emit("agent_start");
            await emit("agent_end", { messages: [{ role: "assistant", stopReason }] });
            if (host === "pi") await emit("agent_settled");
            await heartbeat();
        }
        await emit("session_shutdown");
        assert.deepEqual(messages, []);
        assert.ok(!calls.some(args => args[1] === "stop" || args[1] === "claim-next"));
        assert.ok(calls.some(args => args[1] === "interrupt"));
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
