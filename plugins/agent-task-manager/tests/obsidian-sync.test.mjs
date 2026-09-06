import assert from "node:assert/strict";
import { test } from "node:test";
import { fixture, loadPlugin, copy } from "./support/obsidian-plugin.mjs";

test("Obsidian mappings preserve task leases and explicitly route Job review", () => {
    const { commandFor } = loadPlugin();
    const task = { kind: "task", id: "task_one", status: "TODO" };
    assert.equal(commandFor(task, "BLOCKED")[1], "block");
    assert.match(commandFor(task, "BLOCKED").at(-1), /Obsidian.*TODO -> BLOCKED/);
    assert.equal(commandFor({ ...task, status: "FAILED" }, "TODO")[1], "retry");
    assert.equal(commandFor({ ...task, status: "DONE" }, "TODO")[1], "reopen");
    for (const status of ["IN_PROGRESS", "DONE", "COMPLETED", "", null]) {
        assert.throws(() => commandFor(task, status));
    }
    const job = { kind: "job", id: "job_one", status: "PENDING_REVIEW" };
    assert.equal(commandFor(job, "ACTIVE")[1], "reject");
    assert.equal(commandFor(job, "COMPLETED")[1], "approve");
    assert.equal(commandFor({ ...job, status: "ACTIVE" }, "PENDING_REVIEW")[1], "submit");
    assert.throws(() => commandFor({ ...job, status: "ACTIVE" }, "COMPLETED"));
});

test("Obsidian debounces edits, fences revisions, and ignores CLI projection echoes", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    f.edit("BLOCKED"); f.edit("WAITING_USER");
    await f.engine.flush();
    assert.equal(f.calls.length, 1);
    assert.equal(f.calls[0][1], "wait");
    assert.ok(f.calls[0].includes("--expect-revision"));
    assert.ok(f.calls[0].includes("--idempotency-key"));
    assert.equal(f.notices.length, 0);
    await f.engine.flush();
    assert.equal(f.calls.length, 1);
    assert.equal(f.files.get(f.row.path).custom, "keep");
});

test("Obsidian rolls back failed status and completion date without losing custom properties", async (t) => {
    const f = await fixture({ execute: async () => { throw new Error("active lease"); } });
    t.after(() => f.engine.dispose());
    f.files.get(f.row.path).completedDate = "2026-09-07";
    f.edit("CANCELLED");
    await f.engine.flush();
    assert.equal(f.files.get(f.row.path).status, "TODO");
    assert.equal(f.files.get(f.row.path).completedDate, null);
    assert.equal(f.files.get(f.row.path).custom, "keep");
    assert.match(f.notices[0], /active lease/);
});

test("Obsidian does not replay pre-existing drift or modify copied notes", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    f.files.get(f.row.path).status = "DONE";
    await f.engine.initialize();
    assert.equal(f.files.get(f.row.path).status, "TODO");
    f.engine.observe("Tasks/copied.md", { id: "task_one", revision: 1, status: "BLOCKED" });
    await f.engine.flush();
    assert.equal(f.calls.length, 0);
});

test("Obsidian rejects stale revisions and restores the latest external state", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    f.edit("BLOCKED");
    f.row.revision = 2; f.row.status = "WAITING_USER"; f.row.properties.status = "WAITING_USER";
    await f.engine.flush();
    assert.equal(f.calls.length, 0);
    assert.equal(f.files.get(f.row.path).status, "WAITING_USER");
    assert.match(f.notices[0], /revision|changed/i);
});

test("Obsidian verifies a committed timeout instead of reverting successful work", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    f.io.execute = async () => {
        f.row.status = "BLOCKED"; f.row.properties.status = "BLOCKED"; f.row.revision++;
        throw new Error("timed out");
    };
    f.edit("BLOCKED"); await f.engine.flush();
    assert.equal(f.files.get(f.row.path).status, "BLOCKED");
    assert.equal(f.notices.length, 0);
});

test("Obsidian repairs pending projections without repeating the state command", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    const execute = f.io.execute;
    f.io.execute = async (args) => {
        if (args[0] === "sync") { f.calls.push(copy(args)); throw new Error("read-only disk"); }
        return { ...await execute(args), projection_pending: "read-only disk" };
    };
    f.edit("BLOCKED"); await f.engine.flush();
    assert.equal(f.calls.length, 2);
    assert.equal(f.calls[1][0], "sync");
    assert.equal(f.files.get(f.row.path).status, "BLOCKED");
    assert.match(f.notices[0], /saved|committed/i);
});

test("Obsidian preserves newer edits while a previous command is in flight", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    let release, entered;
    const started = new Promise((resolve) => { entered = resolve; });
    const execute = f.io.execute;
    f.io.execute = async (args) => {
        if (args[1] === "block") { entered(); await new Promise((resolve) => { release = resolve; }); }
        return execute(args);
    };
    f.edit("BLOCKED");
    const pending = f.engine.flush(); await started;
    f.edit("WAITING_USER");
    release(); await pending; await f.engine.flush();
    assert.deepEqual(f.calls.map((c) => c[1]), ["block", "wait"]);
    assert.equal(f.files.get(f.row.path).status, "WAITING_USER");
});

test("Obsidian retains a last confirmed display if CLI reads also fail", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    f.io.snapshot = async () => { throw new Error("CLI unavailable"); };
    f.edit("BLOCKED"); await f.engine.flush();
    assert.equal(f.files.get(f.row.path).status, "TODO");
    assert.match(f.notices[0], /unavailable|confirm/i);
});

test("Obsidian deletion and disposal cancel pending writes", async () => {
    const f = await fixture();
    f.edit("BLOCKED"); f.engine.forget(f.row.path);
    await f.engine.flush(); assert.equal(f.calls.length, 0);
    f.edit("WAITING_USER"); f.engine.dispose();
    await f.engine.flush(); assert.equal(f.calls.length, 0);
});

test("Obsidian refuses prototype names as status commands", () => {
    const { commandFor } = loadPlugin();
    for (const status of ["toString", "constructor", "__proto__"]) {
        assert.throws(() => commandFor({ kind: "task", id: "task_one", status: "TODO" }, status));
    }
});

test("Obsidian preserves acknowledged state if projection and subsequent reads fail", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    f.io.execute = async (args) => {
        if (args[0] === "sync") throw new Error("disk unavailable");
        f.io.snapshot = async () => { throw new Error("read unavailable"); };
        return { ok: true, result: { id: f.row.id, status: "BLOCKED", revision: 2 }, projection_pending: "disk unavailable" };
    };
    f.edit("BLOCKED"); await f.engine.flush();
    assert.equal(f.files.get(f.row.path).status, "BLOCKED");
    assert.equal(f.files.get(f.row.path).revision, 2);
});

test("Obsidian deletion during preflight prevents a state write", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    let release, entered;
    const started = new Promise((resolve) => { entered = resolve; });
    let first = true;
    f.io.snapshot = async () => {
        if (first) { first = false; entered(); await new Promise((resolve) => { release = resolve; }); }
        return copy(f.state);
    };
    f.edit("BLOCKED"); const pending = f.engine.flush(); await started;
    f.engine.forget(f.row.path); release(); await pending;
    assert.equal(f.calls.length, 0);
});

test("Obsidian subprocess uses argument arrays and surfaces structured CLI errors", async () => {
    let reply = { ok: true, schema_version: 1, result: { status: "ACTIVE" } };
    const invocations = [];
    const { runCli } = loadPlugin({}, {}, { "node:child_process": {
        execFile(binary, args, options, callback) {
            invocations.push({ binary, args, options });
            const child = { kill() {} };
            queueMicrotask(() => callback(reply.ok ? null : new Error("exit 1"), typeof reply === "string" ? reply : JSON.stringify(reply), ""));
            return child;
        },
    } });
    const settings = { cliPath: "/bin/task cli", configPath: "/config/$(literal).toml", vaultPath: "/vault" };
    await runCli(settings, ["job", "reject", "job_one", "--reason", "$(literal); text"]);
    assert.equal(invocations[0].binary, settings.cliPath);
    assert.ok(invocations[0].args.includes(settings.configPath));
    assert.ok(invocations[0].args.includes("$(literal); text"));
    assert.equal(invocations[0].options.shell, undefined);
    reply = { ok: false, schema_version: 1, error: { message: "active lease" } };
    await assert.rejects(runCli(settings, ["task", "cancel", "task_one"]), /active lease/);
    reply = "malformed response";
    await assert.rejects(runCli(settings, ["obsidian", "snapshot"]));
});

test("Obsidian ignores a delayed projection echo after the write acknowledgement", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    const snapshot = f.io.snapshot;
    const execute = f.io.execute;
    let echo = false;
    f.io.execute = async (args) => {
        if (args[1] === "block") {
            f.edit("WAITING_USER");
            const result = await execute(args);
            echo = true;
            return result;
        }
        return execute(args);
    };
    f.io.snapshot = async () => {
        if (echo) {
            echo = false;
            f.engine.observe(f.row.path, copy(f.files.get(f.row.path)));
        }
        return snapshot();
    };
    f.edit("BLOCKED"); await f.engine.flush(); await f.engine.flush();
    assert.equal(f.files.get(f.row.path).status, "WAITING_USER");
});

test("Obsidian preserves another note's queued edit across a full projection", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    const second = { ...copy(f.row), id: "task_two", path: "Tasks/Two.md" };
    f.state.notes.push(second);
    f.files.set(second.path, { ...copy(f.files.get(f.row.path)), id: second.id, task_id: second.id });
    await f.engine.initialize();
    const execute = f.io.execute;
    f.io.execute = async (args) => {
        if (args[2] === f.row.id) {
            const result = await execute(args);
            f.files.get(second.path).status = second.status;
            f.engine.observe(second.path, copy(f.files.get(second.path)));
            return result;
        }
        f.calls.push(copy(args));
        second.status = "WAITING_USER";
        second.revision++;
        second.properties.status = second.status;
        return { ok: true, result: copy(second) };
    };
    f.edit("BLOCKED");
    f.files.get(second.path).status = "WAITING_USER";
    f.engine.observe(second.path, copy(f.files.get(second.path)));
    await f.engine.flush();
    assert.equal(second.status, "WAITING_USER");
    assert.equal(f.files.get(second.path).status, "WAITING_USER");
    assert.equal(f.calls.length, 2);
});
