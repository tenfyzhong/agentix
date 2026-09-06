import assert from "node:assert/strict";
import { test } from "node:test";
import { registerExtension, runHook } from "../runtime.mjs";

function fixture(host, runner = async () => ({ result: {} })) {
    const handlers = new Map(), timers = new Map(), calls = [];
    let nextTimer = 0;
    const ctx = (session = "one") => ({
        cwd: "/work",
        sessionManager: { getSessionId: () => session },
        isIdle: () => true,
    });
    registerExtension({
        on: (event, handler) => handlers.set(event, handler),
        registerTool() {},
    }, host, async (args, options) => {
        calls.push({ args, options });
        return runner(args, options);
    }, {
        setInterval: (fn) => { timers.set(++nextTimer, fn); return nextTimer; },
        clearInterval: (id) => timers.delete(id),
    });
    const emit = async (name, event = {}, context = ctx()) => {
        assert.equal(typeof handlers.get(name), "function", `Missing ${host} ${name}`);
        return handlers.get(name)(event, context);
    };
    const aborted = { messages: [{ role: "assistant", stopReason: "aborted" }] };
    const interrupt = async (context = ctx()) => {
        await emit("agent_end", aborted, context);
        if (host === "pi") await emit("agent_settled", {}, context);
    };
    return { emit, interrupt, aborted, timers, calls, ctx };
}

for (const host of ["pi", "omp"]) {
    test(`${host} interruption stops renewal and resumes only on new work`, async () => {
        const f = fixture(host);
        await f.emit("session_start");
        const oldTick = [...f.timers.values()][0];
        await f.interrupt();
        assert.equal(f.calls.at(-1).args[1], "interrupt");
        assert.equal(f.timers.size, 0);
        const count = f.calls.length;
        await oldTick();
        await f.interrupt();
        assert.equal(f.calls.length, count, "stale ticks and duplicate interruptions do nothing");
        await f.emit("before_agent_start");
        assert.equal(f.timers.size, 1);
        assert.equal(f.calls.filter(c => c.args[1] === "session-start").length, 1,
            "continuing never implicitly reclaims a released task");
        await [...f.timers.values()][0]();
        assert.equal(f.calls.at(-1).args[1], "heartbeat");
        await f.emit("session_shutdown");
        assert.equal(f.timers.size, 0);
        assert.equal(f.calls.at(-1).args[1], "session-end");
    });

    test(`${host} normal completion and automatic continuation keep ownership`, async () => {
        const f = fixture(host);
        await f.emit("session_start");
        await f.emit("agent_end", { messages: [{ role: "assistant", stopReason: "stop" }] });
        if (host === "pi") await f.emit("agent_settled");
        assert.equal(f.timers.size, 1);
        if (host === "pi") {
            await f.emit("agent_end", f.aborted);
            assert.equal(f.calls.at(-1).args[1], "session-start", "wait for settle");
            await f.emit("agent_start");
            await f.emit("agent_end", { messages: [{ role: "assistant", stopReason: "stop" }] });
            await f.emit("agent_settled");
        } else {
            await f.emit("agent_end", { ...f.aborted, willContinue: true });
        }
        assert.equal(f.timers.size, 1);
        assert.equal(f.calls.some(c => c.args[1] === "interrupt"), false);
        await f.emit("session_shutdown");
    });

    test(`${host} shutdown fences queued heartbeats and ignores events from a previous session`, async () => {
        const f = fixture(host);
        await f.emit("session_start");
        const oldTick = [...f.timers.values()][0];
        await f.emit("session_start", {}, f.ctx("two"));
        const count = f.calls.length;
        await oldTick();
        await f.interrupt(f.ctx("one"));
        await f.emit("session_shutdown", {}, f.ctx("one"));
        assert.equal(f.calls.length, count);
        assert.equal(f.timers.size, 1);
        await f.emit("session_shutdown", {}, f.ctx("two"));
        assert.equal(f.calls.at(-1).options.session, "two");
        assert.equal(f.timers.size, 0);
    });

    test(`${host} failed cleanup keeps renewal stopped and can retry`, async () => {
        let fail = true;
        const f = fixture(host, async (args) => {
            if (args[1] === "interrupt" && fail) throw new Error("busy database");
            return { result: {} };
        });
        await f.emit("session_start");
        await assert.rejects(f.interrupt(), /busy database/);
        assert.equal(f.timers.size, 0);
        fail = false;
        await f.interrupt();
        assert.equal(f.calls.filter(c => c.args[1] === "interrupt").length, 2);
        await f.emit("session_shutdown");
    });
}

test("Claude only releases a PostToolUseFailure explicitly marked as interrupted", async () => {
    const calls = [];
    const runner = async (args) => { calls.push(args); return { result: {} }; };
    const event = { hook_event_name: "PostToolUseFailure", session_id: "claude", cwd: "/work" };
    for (const is_interrupt of [undefined, false, "true"]) {
        assert.deepEqual(await runHook({ ...event, is_interrupt }, runner), {});
    }
    assert.equal(calls.length, 0, "ordinary tool failures must not renew or release");
    assert.deepEqual(await runHook({ ...event, is_interrupt: true }, runner), {});
    assert.deepEqual(calls, [["hook", "interrupt"]]);
});

for (const host of ["pi", "omp"]) {
    test(`${host} exit cancels in-flight renewal without passing the cancelled signal to cleanup`, async () => {
        let signal;
        const f = fixture(host, async (args, options) => {
            if (args[1] === "heartbeat") {
                signal = options.signal;
                await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
            } else if (args[1] === "session-end") {
                assert.equal(signal.aborted, true);
                assert.equal(options.signal, undefined);
            }
            return { result: {} };
        });
        await f.emit("session_start");
        const pending = [...f.timers.values()][0]();
        await f.emit("session_shutdown");
        await pending;
        assert.equal(f.calls.at(-1).args[1], "session-end");
        assert.equal(f.timers.size, 0);
    });

    test(`${host} new prompts wait for pending interruption cleanup`, async () => {
        let finish, notifyStarted;
        const started = new Promise(resolve => { notifyStarted = resolve; });
        const f = fixture(host, async (args) => {
            if (args[1] === "interrupt") await new Promise(resolve => {
                finish = resolve;
                notifyStarted();
            });
            return { result: {} };
        });
        await f.emit("session_start");
        const pending = f.interrupt();
        await started;
        const prompt = f.emit("before_agent_start");
        assert.equal(f.timers.size, 0);
        assert.equal(f.calls.some(c => c.args[0] === "context"), false);
        finish();
        await Promise.all([pending, prompt]);
        assert.equal(f.timers.size, 1);
        await f.emit("session_shutdown");
    });
}

for (const host of ["pi", "omp"]) {
    test(`${host} repeated SessionStart does not revoke its existing executing lease`, async () => {
        const f = fixture(host);
        await f.emit("session_start");
        const oldTick = [...f.timers.values()][0];
        await f.emit("session_start");
        assert.equal(f.calls.some(c => c.args[1] === "session-end"), false);
        assert.equal(f.timers.size, 1);
        const count = f.calls.length;
        await oldTick();
        assert.equal(f.calls.length, count);
        await f.emit("session_shutdown");
    });
}
