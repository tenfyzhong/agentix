import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { memoryContext } from "../memory.mjs";
import filesystem from "node:fs/promises";
import { syncBuiltinESMExports } from "node:module";
import { runHook, registerExtension } from "../runtime.mjs";

const packet = (revision = 1) => ({ result: { mode: "offline_fts", items: [{ id: "mem-a", revision }], text: `Historical memory\n${JSON.stringify({ id: "mem-a", revision, conclusion: "Use regional endpoints" })}\n` } });
test("receipt publication cannot discard an accepted memory delivery at the deadline", async t => {
    const directory = await mkdtemp(join(tmpdir(), "memory-receipt-deadline-"));
    const originalRename = filesystem.rename;
    const entered = Promise.withResolvers();
    const release = Promise.withResolvers();
    const published = Promise.withResolvers();
    t.mock.method(filesystem, "rename", async (...args) => {
        entered.resolve();
        await release.promise;
        await originalRename(...args);
        published.resolve();
    });
    syncBuiltinESMExports();
    t.mock.timers.enable({ apis: ["setTimeout"] });
    t.after(async () => {
        release.resolve();
        t.mock.restoreAll();
        syncBuiltinESMExports();
        await rm(directory, { recursive: true, force: true });
    });
    const options = { session: "receipt-deadline", cwd: directory };
    const result = memoryContext("regional endpoints", "first", options, async () => packet(),
        { cacheDir: directory, timeoutMs: 20 });
    await entered.promise;
    t.mock.timers.tick(20);
    const text = await result;
    release.resolve();
    await published.promise;
    const next = await memoryContext("regional endpoints", "next", options, async () => packet(),
        { cacheDir: directory });
    assert.ok(text.includes("regional endpoints") || next.includes("regional endpoints"),
        "an unreceived memory must remain available on the next turn");
});
test("memory context persists only revision receipts, deduplicates offline and retains turn retries", async () => {
    const directory = await mkdtemp(join(tmpdir(), "taskix-memory-test-"));
    try {
        const options = { session: "session", cwd: directory };
        const calls = [];
        const runner = async (args, opts) => { calls.push({ args, opts }); return packet(); };
        assert.match(await memoryContext("regional endpoints", "turn-1", options, runner, { cacheDir: directory }), /regional endpoints/);
        assert.match(await memoryContext("regional endpoints", "turn-1", options, runner, { cacheDir: directory }), /regional endpoints/);
        assert.equal(await memoryContext("regional endpoints", "turn-2", options, runner, { cacheDir: directory }), "");
        assert.match(await memoryContext("regional endpoints", "turn-3", options, async () => packet(2), { cacheDir: directory }), /regional endpoints/);
        assert.deepEqual(calls[0].args, ["memory", "context", "regional endpoints", "--turn", "turn-1", "--budget", "6400"]);
        assert.equal(calls[0].opts.session, "session");
        assert.equal(await memoryContext("regional endpoints", "turn-4", options, async () => packet(2), { cacheDir: directory }), "");
    } finally { await rm(directory, { recursive: true, force: true }); }
});
test("memory errors, oversized responses and deadlines never block the main agent", async () => {
    const options = { session: "session" };
    assert.equal(await memoryContext("query", "turn", options, async () => { throw new Error("offline"); }), "");
    assert.equal(await memoryContext("query", "turn", options, async () => ({ result: { text: "x".repeat(7000) } })), "");
    let signal;
    assert.equal(await memoryContext("query", "turn", options, async (_args, opts) => { signal = opts.signal; return new Promise(() => {}); }, { timeoutMs: 20 }), "");
    assert.equal(signal.aborted, true);
});
test("a response arriving after the host deadline cannot suppress the next turn", async t => {
    const directory = await mkdtemp(join(tmpdir(), "memory-late-response-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const options = { session: "late-response", cwd: directory };
    let complete;
    const late = new Promise(resolve => { complete = resolve; });
    assert.equal(await memoryContext("regional endpoints", "lost", options, () => late,
        { cacheDir: directory, timeoutMs: 20 }), "");
    complete(packet());
    await new Promise(resolve => setImmediate(resolve));
    assert.match(await memoryContext("regional endpoints", "received", options, async () => packet(),
        { cacheDir: directory }), /regional endpoints/);
    assert.equal(await memoryContext("regional endpoints", "later", options, async () => packet(),
        { cacheDir: directory }), "");
});
test("Codex and Claude prompt hooks retrieve memory independently of Jev routing", async () => {
    const calls = [];
    const output = await runHook({ hook_event_name: "UserPromptSubmit", session_id: "s", turn_id: "t", prompt: "regional endpoints" }, async args => { calls.push(args); return packet(); }, { env: { TASKIX_JEV_ENABLED: "false", TASKIX_MEMORY_ENABLED: "true" } });
    assert.match(output.hookSpecificOutput.additionalContext, /regional endpoints/);
    assert.deepEqual(calls.map(args => args.slice(0, 2)), [["memory", "context"]]);
});
for (const host of ["pi", "omp"]) test(`${host} retrieves memory before the agent starts`, async () => {
    const handlers = {};
    registerExtension({ on: (name, handler) => { handlers[name] = handler; }, registerTool() {} }, host,
        async args => args[0] === "memory" ? packet() : { result: {} },
        { setInterval: () => 1, clearInterval() {} }, undefined, { env: { TASKIX_JEV_ENABLED: "false", TASKIX_MEMORY_ENABLED: "1" } });
    const ctx = { cwd: tmpdir(), sessionManager: { getSessionId: () => `memory-${host}` } };
    const result = await handlers.before_agent_start({ prompt: "regional endpoints", turn_id: "native-turn" }, ctx);
    assert.match(result.message.content, /regional endpoints/);
});

for (const value of [undefined, "", "false", "0", "TRUE", "yes", " true ", "true", "1"]) {
    test(`memory_environment_${JSON.stringify(value)}_controls_all_hosts_without_config_io`, async t => {
        const enabled = value === "true" || value === "1";
        const env = { TASKIX_JEV_ENABLED: "false", ...(value === undefined ? {} : { TASKIX_MEMORY_ENABLED: value }) };
        const directory = await mkdtemp(join(tmpdir(), "memory-env-"));
        t.after(() => rm(directory, { recursive: true, force: true }));
        const options = { env, cacheDir: directory };
        const calls = [];
        const runner = async args => { if (args[0] === "memory") calls.push(args); return args[0] === "memory" ? packet() : { result: {} }; };
        const output = await runHook({ hook_event_name: "UserPromptSubmit", session_id: "env-hook", turn_id: "t", prompt: "regional endpoints" }, runner, options);
        assert.equal(!!output.hookSpecificOutput?.additionalContext, enabled);
        assert.equal(calls.length, enabled ? 1 : 0);
        if (enabled) await memoryContext("regional endpoints", "cleanup", { session: "env-hook" }, runner, options);
        for (const host of ["pi", "omp"]) {
            const handlers = {};
            calls.length = 0;
            registerExtension({ on: (name, handler) => { handlers[name] = handler; }, registerTool() {} }, host,
                runner, { setInterval: () => 1, clearInterval() {} }, undefined, options);
            const result = await handlers.before_agent_start({ prompt: "regional endpoints", turn_id: "t" },
                { cwd: directory, sessionManager: { getSessionId: () => `env-${host}` } });
            assert.equal(!!result?.message?.content?.includes("Historical memory"), enabled);
            assert.equal(calls.length, enabled ? 1 : 0);
            if (enabled) await memoryContext("regional endpoints", "cleanup", { session: `env-${host}`, cwd: directory }, runner, options);
        }
    });
}
