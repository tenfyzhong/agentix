import assert from "node:assert/strict";
import { test } from "node:test";
import { buildArgs, runHook, registerExtension } from "../runtime.mjs";

test("task tool passes arguments without shell interpolation and fences identity", () => {
    const args = buildArgs(
        ["task", "update", "task_one", "--title", "$(touch nope); title"],
        { session: "one", executor: "agent:one", token: "lease_old" },
    );
    assert.ok(args.includes("$(touch nope); title"));
    assert.equal(args.filter((a) => a === "--session").length, 1);
    assert.throws(() =>
        buildArgs(["task", "claim", "t", "--session=other"], {
            session: "one",
        }),
    );
});

test("hooks distinguish session end from turn completion and provide session context", async () => {
    const calls = [];
    const runner = async (args, options) => {
        calls.push({ args, options });
        return { schema_version: 1, ok: true, result: {} };
    };
    await runHook(
        {
            hook_event_name: "SessionStart",
            session_id: "thr_123",
            cwd: "/work",
        },
        runner,
    );
    assert.deepEqual(calls[0].args, ["hook", "session-start"]);
    assert.equal(calls[0].options.session, "thr_123");
    calls.length = 0;
    await runHook(
        { hook_event_name: "Stop", session_id: "thr_123", cwd: "/work" },
        runner,
    );
    assert.equal(calls[0].args[1], "heartbeat");
    calls.length = 0;
    await runHook(
        { hook_event_name: "SessionEnd", session_id: "thr_123", cwd: "/work" },
        runner,
    );
    assert.equal(calls[0].args[1], "session-end");
});

test("Pi and OMP tool reuses current lease, injects context and cancels heartbeat timer", async () => {
    const handlers = new Map();
    const tools = [];
    const calls = [];
    const timers = new Map();
    const host = {
        on: (event, callback) => handlers.set(event, callback),
        registerTool: (tool) => tools.push(tool),
    };
    const runner = async (args, options) => {
        calls.push({ args, options });
        if (args[0] === "context")
            return {
                schema_version: 1,
                ok: true,
                result: {
                    job_id: "job_one",
                    task_id: "task_one",
                    lease: { token: "lease_one" },
                    documents: { format: "markdown" },
                },
            };
        return { schema_version: 1, ok: true, result: {} };
    };
    registerExtension(host, "pi", runner, {
        setInterval: (fn) => {
            timers.set(1, fn);
            return 1;
        },
        clearInterval: (id) => timers.delete(id),
    });
    const ctx = { cwd: "/work", sessionManager: { getSessionId: () => "s1" } };
    await handlers.get("session_start")({}, ctx);
    assert.equal(timers.size, 1);
    const injected = await handlers.get("before_agent_start")({}, ctx);
    assert.ok(injected.message.content.includes("job_one"));
    const result = await tools[0].execute(
        "call1",
        { args: ["task", "done", "task_one"] },
        undefined,
        undefined,
        ctx,
    );
    assert.equal(result.isError, undefined);
    assert.ok(
        calls.some(
            (c) => c.args[1] === "done" && c.options.token === "lease_one",
        ),
    );
    await tools[0].execute(
        "start1",
        { args: ["task", "start", "task_one"] },
        undefined,
        undefined,
        ctx,
    );
    const start = calls.at(-1);
    assert.equal(start.options.token, "lease_one");
    assert.ok(start.options.idempotencyKey);
    await handlers.get("session_shutdown")({}, ctx);
    assert.equal(timers.size, 0);
    assert.ok(calls.some((c) => c.args[1] === "session-end"));
});

test("tool failures propagate instead of appearing as successful completions", async () => {
    let tool;
    const host = {
        on() {},
        registerTool: (t) => {
            tool = t;
        },
    };
    registerExtension(host, "omp", async () => {
        throw new Error("conflict: stale lease");
    });
    await assert.rejects(
        tool.execute(
            "call",
            { args: ["task", "done", "t"] },
            undefined,
            undefined,
            { cwd: "/work", sessionManager: { getSessionId: () => "s" } },
        ),
        /stale lease/,
    );
});

test("a conflicting explicit lease is not replaced with another task's lease", async () => {
    let tool;
    const calls = [];
    const host = {
        on() {},
        registerTool: (value) => {
            tool = value;
        },
    };
    registerExtension(host, "pi", async (args, options) => {
        calls.push({ args, options });
        return {
            schema_version: 1,
            ok: true,
            result: { task_id: "task_other", lease: { token: "other_token" } },
        };
    });
    await tool.execute(
        "call",
        { args: ["task", "done", "task_target"] },
        undefined,
        undefined,
        { cwd: "/work", sessionManager: { getSessionId: () => "one" } },
    );
    assert.equal(calls.at(-1).options.token, undefined);
});

test("heartbeat callbacks serialize renewals, report failure and follow session switches", async () => {
    const handlers = new Map(),
        timers = new Map(),
        calls = [],
        warnings = [];
    let nextTimer = 0,
        session = "first",
        finish;
    const host = {
        on: (name, fn) => handlers.set(name, fn),
        registerTool() {},
    };
    registerExtension(
        host,
        "pi",
        async (args, options) => {
            calls.push({ args, options });
            if (args[1] === "heartbeat")
                await new Promise((resolve, reject) => {
                    finish = { resolve, reject };
                });
            return { schema_version: 1, ok: true, result: {} };
        },
        {
            setInterval: (fn) => {
                timers.set(++nextTimer, fn);
                return nextTimer;
            },
            clearInterval: (id) => timers.delete(id),
        },
    );
    const ctx = {
        cwd: "/work",
        sessionManager: { getSessionId: () => session },
        ui: { notify: (message) => warnings.push(message) },
    };
    await handlers.get("session_start")({}, ctx);
    const tick = timers.get(1);
    const pending = tick();
    await tick();
    assert.equal(calls.filter((c) => c.args[1] === "heartbeat").length, 1);
    finish.reject(new Error("temporary process failure"));
    await pending;
    assert.ok(warnings[0].includes("temporary process failure"));
    const retry = tick();
    finish.resolve();
    await retry;
    session = "second";
    await handlers.get("session_start")({}, ctx);
    assert.equal(timers.size, 1);
    assert.ok(!timers.has(1));
    assert.ok(
        calls.some(
            (c) => c.args[1] === "session-end" && c.options.session === "first",
        ),
    );
    const renewed = timers.get(2)();
    assert.equal(calls.at(-1).options.session, "second");
    finish.resolve();
    await renewed;
    await handlers.get("session_shutdown")({}, ctx);
    assert.equal(timers.size, 0);
});

test("transport failure retries retain the original lease even if current context is gone", async () => {
    let tool,
        committed = false;
    const tokens = [];
    registerExtension(
        {
            on() {},
            registerTool: (value) => {
                tool = value;
            },
        },
        "omp",
        async (args, options) => {
            if (args[0] === "context")
                return {
                    result: committed
                        ? {}
                        : { task_id: "task_one", lease: { token: "original" } },
                };
            tokens.push(options.token);
            if (!committed) {
                committed = true;
                throw new Error("reply lost after commit");
            }
            return { schema_version: 1, ok: true, result: { status: "DONE" } };
        },
    );
    const ctx = {
        cwd: "/work",
        sessionManager: { getSessionId: () => "session" },
    };
    const invoke = () =>
        tool.execute(
            "same-call",
            {
                args: [
                    "task",
                    "done",
                    "task_one",
                    "--idempotency-key=explicit-key",
                ],
            },
            undefined,
            undefined,
            ctx,
        );
    await assert.rejects(invoke(), /reply lost/);
    assert.equal((await invoke()).details.result.status, "DONE");
    assert.deepEqual(tokens, ["original", "original"]);
});

test("retry tokens are scoped by the exact session and idempotency key", async () => {
    let tool,
        session = "a:b";
    const tokens = [];
    registerExtension(
        {
            on() {},
            registerTool(value) {
                tool = value;
            },
        },
        "pi",
        async (args, options) => {
            if (args[0] === "context")
                return {
                    result: {
                        task_id: "task_one",
                        lease: { token: `token:${session}` },
                    },
                };
            tokens.push(options.token);
            return { result: {} };
        },
    );
    const ctx = {
        cwd: "/work",
        sessionManager: { getSessionId: () => session },
    };
    const invoke = (key) =>
        tool.execute(
            "call",
            { args: ["task", "done", "task_one", "--idempotency-key", key] },
            undefined,
            undefined,
            ctx,
        );
    await invoke("c");
    session = "a";
    await invoke("b:c");
    assert.deepEqual(tokens, ["token:a:b", "token:a"]);
});
