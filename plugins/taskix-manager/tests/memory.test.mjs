import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { memoryContext, memoryConfigured } from "../memory.mjs";
import { runHook, registerExtension } from "../runtime.mjs";

const packet = (revision = 1) => ({ result: { mode: "offline_fts", items: [{ id: "mem-a", revision }], text: `Historical memory\n${JSON.stringify({ id: "mem-a", revision, conclusion: "Use regional endpoints" })}\n` } });
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
test("Codex and Claude prompt hooks retrieve memory independently of Jev routing", async t => {
    const memoryConfigPath = await enabledConfig(t);
    const calls = [];
    const output = await runHook({ hook_event_name: "UserPromptSubmit", session_id: "s", turn_id: "t", prompt: "regional endpoints" }, async args => { calls.push(args); return packet(); }, { memoryConfigPath, env: { TASKIX_JEV_ENABLED: "false" } });
    assert.match(output.hookSpecificOutput.additionalContext, /regional endpoints/);
    assert.deepEqual(calls.map(args => args.slice(0, 2)), [["memory", "context"]]);
});
for (const host of ["pi", "omp"]) test(`${host} retrieves memory before the agent starts`, async t => {
    const memoryConfigPath = await enabledConfig(t);
    const handlers = {};
    registerExtension({ on: (name, handler) => { handlers[name] = handler; }, registerTool() {} }, host,
        async args => args[0] === "memory" ? packet() : { result: {} },
        { setInterval: () => 1, clearInterval() {} }, undefined, { memoryConfigPath, env: { TASKIX_JEV_ENABLED: "false" } });
    const ctx = { cwd: tmpdir(), sessionManager: { getSessionId: () => `memory-${host}` } };
    const result = await handlers.before_agent_start({ prompt: "regional endpoints", turn_id: "native-turn" }, ctx);
    assert.match(result.message.content, /regional endpoints/);
});

test("memory activation uses the shared TOML file without starting a CLI when disabled", async () => {
    const dir = await mkdtemp(join(tmpdir(), "memory-config-"));
    const path = join(dir, "config.toml");
    try {
        assert.equal(await memoryConfigured(path), false);
        await writeFile(path, 'memory.enabled = true\n[memory.agent]\nmodel = 42\n');
        assert.equal(await memoryConfigured(path), true);
        await writeFile(path, '[memory]\nenabled = false\n');
        assert.equal(await memoryConfigured(path), false);
    } finally { await rm(dir, { recursive: true, force: true }); }
});

async function enabledConfig(t) {
    const dir = await mkdtemp(join(tmpdir(), "memory-enabled-"));
    t.after(() => rm(dir, { recursive: true, force: true }));
    const path = join(dir, "config.toml");
    await writeFile(path, '[memory]\nenabled=true\n');
    return path;
}
