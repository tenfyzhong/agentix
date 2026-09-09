import assert from "node:assert/strict";
import { test } from "node:test";
import { fixture, loadPlugin, copy, connectionFixture } from "./support/obsidian-plugin.mjs";

test("Obsidian starts without a snapshot and bounds its cache while querying only requested IDs", async (t) => {
    const { SyncEngine } = loadPlugin();
    const ids = [];
    const engine = new SyncEngine({
        connection: async () => ({ documents: { directory: "Tasks" } }),
        lookup: async (id) => {
            ids.push(id);
            return { id, kind: "task", path: `Tasks/${id}.md`, revision: 1, status: "TODO", properties: {} };
        },
        read: async (path) => ({ id: path.slice(6, -3), revision: 1, status: "TODO" }),
        notice: (message) => assert.fail(message),
        snapshot: async () => assert.fail("must not fetch the full snapshot"),
        execute: async () => assert.fail("unchanged statuses must not write"),
    });
    t.after(() => engine.dispose());
    await engine.initialize();
    assert.equal(ids.length, 0);
    assert.equal(engine.notes.size, 0);
    for (let i = 0; i < 200; i++) {
        const id = `task_${i}`;
        engine.observe(`Tasks/${id}.md`, { id, task_id: id, revision: 1, status: "TODO" });
        await engine.flush();
    }
    assert.deepEqual(ids, Array.from({ length: 200 }, (_, i) => `task_${i}`));
    assert.ok(engine.notes.size <= 128);
    assert.equal(engine.generations.size, 0);
    assert.equal(engine.pending.size, 0);
    engine.dispose();
    assert.equal(engine.notes.size, 0);
});

test("Obsidian connection requests only configuration", async (t) => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    const connecting = f.plugin.connect();
    assert.deepEqual(copy(f.requests[0].args.slice(-2)), ["obsidian", "connection"]);
    f.reply(); await connecting;
});

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

async function jobFixture(t) {
    const f = await fixture();
    t.after(() => f.engine.dispose());
    Object.assign(f.row, { kind: "job", id: "job_one", status: "ACTIVE" });
    f.row.properties.status = "ACTIVE";
    Object.assign(f.files.get(f.row.path), { id: "job_one", status: "ACTIVE" });
    await f.engine.initialize();
    return f;
}

test("Obsidian cancels Jobs with canonical or whitespace-padded status strings", async (t) => {
    for (const target of ["CANCELLED", "CANCELLED\n", " \tCANCELLED\r\n"]) {
        await t.test(JSON.stringify(target), async (t) => {
            const f = await jobFixture(t);
            f.edit(target);
            await f.engine.flush();
            assert.equal(f.calls.length, 1);
            assert.deepEqual(f.calls[0].slice(0, 3), ["job", "cancel", "job_one"]);
            assert.equal(f.row.status, "CANCELLED");
            assert.equal(f.files.get(f.row.path).status, "CANCELLED");
            assert.deepEqual(f.notices, []);
            await f.engine.flush();
            assert.equal(f.calls.length, 1);
        });
    }
});

test("Obsidian restores rejected padded Job edits without replaying them", async (t) => {
    const f = await jobFixture(t);
    let attempts = 0;
    f.io.execute = async () => { attempts++; throw new Error("active lease"); };
    f.edit("CANCELLED\n");
    await f.engine.flush();
    assert.equal(attempts, 1);
    assert.equal(f.files.get(f.row.path).status, "ACTIVE");
    assert.match(f.notices[0], /active lease/);
    await f.engine.flush();
    assert.equal(attempts, 1);
    assert.equal(f.engine.pending.size, 0);
});

test("Obsidian preserves a newer edit while padded cancellation is rejected", async (t) => {
    const f = await jobFixture(t);
    let release, entered;
    const started = new Promise((resolve) => { entered = resolve; });
    f.io.execute = async (args) => {
        f.calls.push(copy(args));
        if (args[1] === "cancel") {
            entered();
            await new Promise((resolve) => { release = resolve; });
            throw new Error("active lease");
        }
        assert.equal(args[1], "submit");
        f.row.status = "PENDING_REVIEW";
        f.row.properties.status = "PENDING_REVIEW";
        f.row.revision++;
        return { result: copy(f.row) };
    };
    f.edit("CANCELLED\n");
    const pending = f.engine.flush();
    await started;
    f.edit(" PENDING_REVIEW\n");
    release();
    await pending;
    await f.engine.flush();
    assert.deepEqual(f.calls.map((args) => args[1]), ["cancel", "submit"]);
    assert.equal(f.files.get(f.row.path).status, "PENDING_REVIEW");
    assert.equal(f.engine.pending.size, 0);
});

test("Obsidian normalizes unchanged Job status without a mutation", async (t) => {
    const f = await jobFixture(t);
    f.edit(" ACTIVE\n");
    await f.engine.flush();
    assert.equal(f.calls.length, 0);
    assert.equal(f.files.get(f.row.path).status, "ACTIVE");
    assert.deepEqual(f.notices, []);
});

test("Obsidian keeps status case and type validation", async (t) => {
    for (const target of ["cancelled\n", ["CANCELLED"], null, 1]) {
        await t.test(JSON.stringify(target), async (t) => {
            const f = await jobFixture(t);
            f.edit(target);
            await f.engine.flush();
            assert.equal(f.calls.length, 0);
            assert.equal(f.files.get(f.row.path).status, "ACTIVE");
            assert.match(f.notices[0], /Unsupported job transition/);
        });
    }
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
    f.files.get(f.row.path).completed_at = "2026-09-07";
    f.edit("CANCELLED");
    await f.engine.flush();
    assert.equal(f.files.get(f.row.path).status, "TODO");
    assert.equal(f.files.get(f.row.path).completed_at, null);
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
    assert.deepEqual(f.calls[1], ["sync", "--pending"]);
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
    f.io.lookup = async () => { throw new Error("CLI unavailable"); };
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

test("Obsidian releases tracking when an edit is reverted before debounce", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    f.edit("BLOCKED"); f.edit("TODO");
    await f.engine.flush();
    assert.equal(f.calls.length, 0);
    assert.equal(f.engine.generations.size, 0);
    assert.equal(f.engine.timers.size, 0);
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
        f.io.lookup = async () => { throw new Error("read unavailable"); };
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
    f.io.lookup = async () => {
        if (first) { first = false; entered(); await new Promise((resolve) => { release = resolve; }); }
        return copy(f.row);
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
    const lookup = f.io.lookup;
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
    f.io.lookup = async () => {
        if (echo) {
            echo = false;
            f.engine.observe(f.row.path, copy(f.files.get(f.row.path)));
        }
        return lookup(f.row.id);
    };
    f.edit("BLOCKED"); await f.engine.flush(); await f.engine.flush();
    assert.equal(f.files.get(f.row.path).status, "WAITING_USER");
});

for (const cached of [true, false]) test(`Obsidian preserves another note's queued edit across a full projection (cached=${cached})`, async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    const second = { ...copy(f.row), id: "task_two", path: "Tasks/Two.md" };
    f.state.notes.push(second);
    f.files.set(second.path, { ...copy(f.files.get(f.row.path)), id: second.id, task_id: second.id });
    await f.engine.initialize();
    if (!cached) f.engine.notes.clear();
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

test("Obsidian Connect button shows progress, then success, and enables itself again", async (t) => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    const checking = f.button.click();
    assert.equal(f.button.disabled, true);
    assert.equal(f.button.text, "Checking...");
    assert.match(f.notices[0].message, /Checking/);
    f.reply(); await checking;
    assert.equal(f.button.disabled, false);
    assert.equal(f.button.text, "Connect");
    assert.equal(f.notices[0].hidden, true);
    assert.match(f.notices.at(-1).message, /Connected to taskix/);
    assert.equal(f.plugin.engine.ready, true);
});

test("Obsidian connection failure is visible and allows another attempt", async (t) => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    const checking = f.button.click(); f.reply("Executable is unavailable"); await checking;
    assert.equal(f.button.disabled, false);
    assert.equal(f.button.text, "Connect");
    assert.equal(f.notices[0].hidden, true);
    assert.match(f.notices.at(-1).message, /Executable is unavailable/);
    assert.equal(f.plugin.engine.ready, false);
    const retry = f.button.click(); f.reply(); await retry;
    assert.match(f.notices.at(-1).message, /Connected to taskix/);
});

test("Obsidian manual connection command reports success while startup stays quiet", async (t) => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    const startup = f.plugin.connect(); f.reply(); await startup;
    assert.equal(f.notices.length, 0);
    const checking = f.commands[0].callback();
    f.reply(); await checking;
    assert.match(f.notices.at(-1).message, /Connected to taskix/);
});

test("Obsidian connection setup errors restore the button and surface the error", async (t) => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    f.plugin.app.vault.adapter.getBasePath = () => { throw new Error("Vault path unavailable"); };
    await f.button.click();
    assert.equal(f.button.disabled, false);
    assert.match(f.notices.at(-1).message, /Vault path unavailable/);
    assert.equal(f.notices[0].hidden, true);
});

test("committed fallback does not restore Task completedDate", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    delete f.row.properties.completedDate;
    f.io.execute = async () => {
        f.io.lookup = async () => { throw new Error("offline after commit"); };
        return { result: { id: f.row.id, status: "CANCELLED", revision: 2, completed_at: null, updated_at: 1788566400 } };
    };
    f.edit("CANCELLED"); await f.engine.flush();
    assert.equal(Object.hasOwn(f.engine.notes.get(f.row.path).properties, "completedDate"), false);
    assert.match(f.engine.notes.get(f.row.path).properties.updated_at, /[+-]\d{2}:\d{2}$/);
});

test("real plugin patch removes duplicate timestamps from Tasks and Jobs", async () => {
    const f = await connectionFixture();
    const connecting = f.plugin.connect();
    const props = {id:"one",revision:1,status:"DONE",completedDate:"legacy",dateCreated:"created",dateModified:"modified",created:"old",updated:"old",custom:"keep"};
    f.plugin.app.vault.getAbstractFileByPath = () => f.file;
    f.plugin.app.fileManager = {processFrontMatter:async (_file, patch) => patch(props)};
    await f.plugin.engine.io.patch("note.md", {id:"one",revision:1,status:"DONE"}, {status:"DONE",created_at:"canonical",updated_at:"latest",completed_at:"finished"}, {kind:"task"});
    for (const key of ["completedDate","dateCreated","dateModified","created","updated"]) assert.equal(Object.hasOwn(props,key),false,key);
    assert.equal(props.created_at,"canonical");
    assert.equal(props.custom,"keep");
    props.completedDate = "legacy-job";
    props.dateModified = "legacy-update";
    await f.plugin.engine.io.patch("note.md", {id:"one",revision:1,status:"DONE"}, {completed_at:"job-date"}, {kind:"job"});
    assert.equal(props.completed_at,"job-date");
    assert.equal(Object.hasOwn(props,"completedDate"),false);
    assert.equal(Object.hasOwn(props,"dateModified"),false);
    f.reply(); await connecting; f.plugin.onunload();
});

test("Obsidian ignores events outside the configured taskix directory", async (t) => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    const connecting = f.plugin.connect(); f.reply(); await connecting;
    for (const path of ["Daily/New.md", "11-Agents-copy/Task.md", "Task.md"]) {
        f.metadataEvents.get("changed")({ path }, "<!-- taskix:inbox:start project=prj_copy -->", {
            frontmatter: { id: "task_copy", status: "TODO", revision: 1 },
        });
        f.vaultEvents.get("rename")({ path }, path.replace(".md", "-old.md"));
        f.vaultEvents.get("delete")({ path });
    }
    assert.equal(f.plugin.engine.pending.size, 0);
    await f.plugin.engine.flush();
    assert.equal(f.requests.length, 1);
    assert.equal(f.notices.length, 0);
});

test("Obsidian does not retry failed startup on vault file events", async (t) => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    const connecting = f.plugin.connect(); f.reply("unrecognized subcommand 'snapshot'"); await connecting;
    for (const path of ["Daily/New.md", "11-Agents/New.md"]) {
        f.vaultEvents.get("rename")({ path }, "Untitled.md");
    }
    assert.equal(f.plugin.engine.pending.size, 0);
    await f.plugin.engine.flush();
    assert.equal(f.requests.length, 1);
    assert.equal(f.notices.length, 1);
});

test("Obsidian queries new notes and moved files by ID in configured directories", async (t) => {
    for (const directory of ["11-Agents", "Nested/Agent Tasks/", "."]) {
        const f = await connectionFixture(directory); t.after(() => f.plugin.onunload());
        const connecting = f.plugin.connect(); f.reply(); await connecting;
        const prefix = directory === "." ? "" : directory.replace(/\/$/, "") + "/";
        const managed = prefix + "Projects/Demo/Tasks/New.md";
        const properties = { id: "task_new", task_id: "task_new", revision: 1, status: "TODO" };
        f.metadataEvents.get("changed")({ path: managed }, "", { frontmatter: properties });
        let refreshing = f.plugin.engine.flush();
        assert.deepEqual(copy(f.requests.at(-1).args.slice(-3)), ["obsidian", "show", "task_new"]);
        f.reply(); await refreshing;
        f.file.path = managed;
        f.plugin.app.vault.read = async () => `---\n${JSON.stringify(properties)}\n---\n`;
        await f.vaultEvents.get("rename")(f.file, "Daily/New.md");
        refreshing = f.plugin.engine.flush();
        assert.deepEqual(copy(f.requests.at(-1).args.slice(-3)), ["obsidian", "show", "task_new"]);
        f.reply(); await refreshing;
        f.plugin.engine.observe(managed, properties);
        f.file.path = "Daily/New.md";
        await f.vaultEvents.get("rename")(f.file, managed);
        assert.equal(f.plugin.engine.pending.has(managed), false);
        refreshing = f.plugin.engine.flush();
        if (directory === ".") { f.reply(); }
        await refreshing;
        assert.ok(f.requests.every((request) => !request.args.includes("snapshot")));
        assert.equal(f.notices.length, 0);
    }
});

test("Obsidian ignores an uncached copied note after verifying its authoritative path", async (t) => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    let queried;
    f.io.lookup = async (id) => { queried = id; return copy(f.row); };
    f.engine.observe("Tasks/copied.md", { id: f.row.id, task_id: f.row.id, revision: 1, status: "BLOCKED" });
    await f.engine.flush();
    assert.equal(queried, f.row.id);
    assert.equal(f.calls.length, 0);
    assert.equal(f.engine.notes.has("Tasks/copied.md"), false);
});
