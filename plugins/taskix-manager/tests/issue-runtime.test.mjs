import assert from "node:assert/strict";
import { test } from "node:test";
import { registerExtension, runHook } from "../runtime.mjs";

const claimed = { claimed: true, entry: { id: "issue_one", job_id: "job_one", revision: 2 }, job: { id: "job_one" } };

test("Stop hooks leave queued Issue work for an explicit user request", async () => {
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
    const runner = async args => ({ result: args[0] === "context" ? { issue_cancellations: [{ id: "issue_one", job_id: "job_one" }] } : {} });
    const event = { session_id: "session", cwd: "/work" };
    const result = await runHook({ ...event, hook_event_name: "PreToolUse" }, runner);
    assert.match(result.hookSpecificOutput.additionalContext, /cancelled/i);
    assert.match(result.hookSpecificOutput.additionalContext, /job_one/);
    assert.deepEqual(await runHook({ ...event, hook_event_name: "Interrupt" }, runner), {});
});

for (const host of ["pi", "omp"]) {
    test(`${host} completion, idle events and heartbeats never take queued Issue work`, async () => {
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
        assert.ok(!calls.some(args => args[1] === "stop" || args[0] === "job" && args[1] === "create"));
        assert.ok(calls.some(args => args[1] === "interrupt"));
    });


}

test("prompt hooks expose TODO candidates and instruct AI semantic selection", async () => {
    const todos = [{ id: "issue_one", requirement: "Fix login", status: "TODO" }, { id: "issue_two", requirement: "Add tests", status: "TODO" }];
    const calls = [];
    const runner = async args => {
        calls.push(args);
        return { result: args[0] === "context" ? { issue_candidates: todos } : {} };
    };
    const result = await runHook({ hook_event_name: "PreToolUse", session_id: "session", cwd: "/work" }, runner);
    const context = result.hookSpecificOutput.additionalContext;
    assert.match(context, /semantic/i);
    assert.match(context, /--issue/);
    for (const entry of todos) assert.ok(context.includes(entry.requirement));
    assert.ok(!calls.some(args => args[0] === "job" || args[0] === "job" && args[1] === "create"));
});
