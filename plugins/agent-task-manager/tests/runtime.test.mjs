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
