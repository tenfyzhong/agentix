import assert from "node:assert/strict";
import { test } from "node:test";
import { connectionFixture } from "./support/obsidian-plugin.mjs";

async function memoryFixture(t, globals = {}) {
    const f = await connectionFixture("11-Agents", globals);
    t.after(() => f.plugin.onunload());
    const connected = f.plugin.connect(); f.reply(); await connected;
    f.file.path = `11-Agents/Projects/demo/Memory/mem_${"a".repeat(32)}.md`;
    f.content = "Canonical database memory";
    f.plugin.app.vault.getAbstractFileByPath = () => f.file;
    f.plugin.app.vault.read = async () => f.content;
    f.plugin.app.vault.process = async (_file, transform) => { f.content = transform(f.content); };
    f.calls = [];
    f.plugin.engine.io.execute = async args => {
        f.calls.push([...args]);
        return { result: { path: f.file.path, text: "Canonical database memory", revision: 2 } };
    };
    return f;
}

test("memory save reports read-only error and rolls back all text without trusting frontmatter", async t => {
    const f = await memoryFixture(t);
    f.content = "edited body and invalid frontmatter";
    await f.plugin.checkMemory(f.file);
    assert.equal(f.content, "Canonical database memory");
    assert.match(f.notices.at(-1).message, /read-only.*restored/i);
    assert.deepEqual(f.calls, [["memory", "document", f.file.path]]);
    await f.plugin.checkMemory(f.file);
    assert.equal(f.notices.length, 1);
});

test("memory lookup failure reports error without replacing local bytes", async t => {
    const f = await memoryFixture(t);
    f.content = "local edit";
    f.plugin.engine.io.execute = async () => { throw new Error("database unavailable"); };
    await f.plugin.checkMemory(f.file);
    assert.equal(f.content, "local edit");
    assert.match(f.notices.at(-1).message, /could not restore.*database unavailable/i);
});

test("memory protection excludes Recovery copies and ordinary notes", async t => {
    const f = await memoryFixture(t);
    for (const path of ["11-Agents/Projects/demo/Memory/Recovery/mem_a.md", "11-Agents/Projects/demo/Tasks/task_a.md", "Elsewhere/Memory/mem_a.md"]) {
        f.file.path = path;
        await f.plugin.checkMemory(f.file);
    }
    assert.equal(f.calls.length, 0);
});

test("memory rollback does not overwrite a concurrent edit and retries its latest event", async t => {
    const f = await memoryFixture(t);
    f.content = "first edit";
    let release;
    f.plugin.engine.io.execute = async () => {
        await new Promise(resolve => { release = resolve; });
        return { result: { path: f.file.path, text: "Canonical database memory" } };
    };
    const first = f.plugin.checkMemory(f.file);
    await new Promise(resolve => setImmediate(resolve));
    f.content = "second edit";
    const second = f.plugin.checkMemory(f.file);
    release();
    await new Promise(resolve => setImmediate(resolve));
    release();
    await Promise.all([first, second]);
    assert.equal(f.content, "Canonical database memory");
    assert.equal(f.notices.length, 1);
});

test("memory lookup cannot restore another path or write after plugin unload", async t => {
    const f = await memoryFixture(t);
    f.content = "local edit";
    f.plugin.engine.io.execute = async () => ({ result: { path: "foreign.md", text: "wrong memory" } });
    await f.plugin.checkMemory(f.file);
    assert.equal(f.content, "local edit");
    assert.match(f.notices.at(-1).message, /Invalid authoritative memory/);
    let release;
    f.plugin.engine.io.execute = async () => {
        await new Promise(resolve => { release = resolve; });
        return { result: { path: f.file.path, text: "Canonical database memory" } };
    };
    const pending = f.plugin.checkMemory(f.file);
    await new Promise(resolve => setImmediate(resolve));
    f.plugin.onunload(); release(); await pending;
    assert.equal(f.content, "local edit");
    assert.equal(f.notices.length, 1);
});

async function flushEvents() { await new Promise(resolve => setImmediate(resolve)); }

test("memory saves debounce until typing stops and notices are limited per file", async t => {
    t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 100000 });
    const f = await memoryFixture(t, { Date });
    const save = () => { f.content = "edit"; f.vaultEvents.get("modify")(f.file); };
    for (let i = 0; i < 5; i++) {
        save(); t.mock.timers.tick(100); await flushEvents();
    }
    assert.equal(f.calls.length, 0);
    t.mock.timers.tick(649); await flushEvents();
    assert.equal(f.calls.length, 0);
    t.mock.timers.tick(1); await flushEvents();
    assert.equal(f.calls.length, 1);
    assert.equal(f.content, "Canonical database memory");
    assert.equal(f.notices.length, 1);
    save(); t.mock.timers.tick(750); await flushEvents();
    assert.equal(f.content, "Canonical database memory");
    assert.equal(f.notices.length, 1);
    t.mock.timers.tick(30000);
    save(); t.mock.timers.tick(750); await flushEvents();
    assert.equal(f.notices.length, 2);
    // Preserve the managed directory; change only the memory ID.
    f.file.path = `11-Agents/Projects/demo/Memory/mem_${"b".repeat(32)}.md`;
    save(); t.mock.timers.tick(750); await flushEvents();
    assert.equal(f.notices.length, 3);
});

test("unloading cancels pending memory checks", async t => {
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const f = await memoryFixture(t);
    f.content = "edit";
    f.vaultEvents.get("modify")(f.file);
    f.plugin.onunload();
    t.mock.timers.tick(1000); await flushEvents();
    assert.equal(f.calls.length, 0);
    assert.equal(f.content, "edit");
});
