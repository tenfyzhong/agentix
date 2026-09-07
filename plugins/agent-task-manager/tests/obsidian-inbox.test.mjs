import assert from "node:assert/strict";
import { test } from "node:test";
import { loadPlugin, copy } from "./support/obsidian-plugin.mjs";

const project = "prj_demo", filePath = "Projects/demo/Inbox.md";
const id = "inbox_00000000000000000000000000000001";
const secondId = "inbox_00000000000000000000000000000002";
const header = (entryId = id, status = "COMPLETED", check = "x", revision = 3) =>
    `- [${check}] Title <!-- taskcli:entry:${entryId} --> <!-- taskcli:entry-state ${status} revision=${revision} --> · [[Job]]`;
const document = (body) => `# Inbox\n\n<!-- taskcli:inbox:start project=${project} -->\n${body}\n<!-- taskcli:inbox:end -->\n\nKeep notes.\n`;

test("Inbox checkboxes map exactly to the five aligned states regardless of the old receipt", () => {
    const { parseInbox, patchInbox, commandFor } = loadPlugin();
    for (const [check, status] of [[" ", "TODO"], ["/", "ACTIVE"], ["r", "PENDING_REVIEW"], ["x", "COMPLETED"], ["-", "CANCELLED"]]) {
        const source = document(header(id, "IN_PROGRESS", check));
        const row = parseInbox(source, project)[0];
        assert.equal(row.status, status);
        const note = { id, project_id: project, kind: "inbox", status: "TODO" };
        assert.deepEqual(copy(commandFor(note, status)), ["inbox", "set-status", id, "--status", status]);
        const restored = patchInbox(document(header(id, "TODO", " ")), note, { id, revision: 3, status: "TODO" }, { status, revision: 4 });
        assert.ok(restored.includes(`- [${check}] Title`));
        assert.ok(restored.includes(`entry-state ${status} revision=4`));
    }
});

test("Inbox parser scopes identities to managed top-level entries and rejects ambiguity", () => {
    const { parseInbox } = loadPlugin();
    const source = document(`${header()}\n  Details\n  ${header(secondId)}\n\n~~~md\n${header(secondId)}\n~~~`);
    const rows = parseInbox(source, project);
    assert.equal(rows.length, 1);
    assert.equal(rows[0].id, id);
    assert.equal(rows[0].status, "COMPLETED");
    assert.equal(rows[0].revision, 3);
    assert.equal(parseInbox(document(header(id, "IN_PROGRESS", " ")), project)[0].status, "TODO");
    assert.equal(parseInbox(document(header(id, "COMPLETED", " ")), project)[0].status, "TODO");
    assert.throws(() => parseInbox(document(`${header()}\n${header()}`), project), /duplicate/i);
    assert.throws(() => parseInbox(source, "prj_other"), /project|region/i);
    assert.throws(() => parseInbox(document("```md\n" + header()), project), /fence/i);
});

test("Legacy pending review markers are read safely and restored using r", () => {
    const { parseInbox, patchInbox } = loadPlugin();
    const source = document(header(id, "PENDING_REVIEW", "p") + "\n  Keep - [p] examples.");
    const row = parseInbox(source, project)[0];
    assert.equal(row.status, "PENDING_REVIEW");
    const next = patchInbox(source, {id, project_id:project}, row, {status:"PENDING_REVIEW",revision:row.revision});
    assert.equal(next, source.replace("- [p] Title", "- [r] Title"));
    const attempted = next.replace("- [r] Title", "- [x] Title");
    assert.equal(patchInbox(attempted, {id,project_id:project}, parseInbox(attempted,project)[0],
        {status:"PENDING_REVIEW",revision:row.revision}), next);
});

test("Inbox rollback changes only the matching checkbox and receipt with compare-and-swap", () => {
    const { patchInbox } = loadPlugin();
    const source = document(`${header(id, "COMPLETED", " ")}\n  Authored details.\n${header(secondId)}`);
    const note = { id, project_id: project };
    const expected = { id, revision: 3, status: "TODO" };
    const next = patchInbox(source, note, expected, { status: "COMPLETED", revision: 3 });
    assert.equal(next, source.replace("- [ ] Title", "- [x] Title"));
    assert.equal(patchInbox(source, note, { ...expected, revision: 2 }, { status: "COMPLETED", revision: 3 }), source);
    assert.equal(patchInbox(source, note, { ...expected, status: "CANCELLED" }, { status: "COMPLETED", revision: 3 }), source);
});

async function inboxFixture(t, { fail = false, offline = false } = {}) {
    const { SyncEngine, parseInbox, patchInbox } = loadPlugin();
    const rows = [id, secondId].map((entryId) => ({ kind: "inbox", id: entryId, project_id: project, path: filePath,
        status: "COMPLETED", revision: 3, properties: { status: "COMPLETED", revision: 3 } }));
    let source = document(rows.map((r) => header(r.id, r.status, offline ? " " : "x")).join("\n"));
    const calls = [], notices = [];
    let engine;
    const io = {
        connection: async () => ({ documents: { directory: "." } }),
        lookup: async (entryId) => copy(rows.find((row) => row.id === entryId) || null),
        openNotes: () => [{ path: filePath, source }],
        read: async (_path, note) => parseInbox(source, project).find((r) => r.id === note.id),
        patch: async (_path, expected, properties, note) => {
            source = patchInbox(source, note, expected, properties);
            engine.observeInbox(filePath, source);
        },
        execute: async (args) => {
            calls.push(copy(args));
            if (fail) throw new Error("Job is archived");
            const row = rows.find((r) => r.id === args[2]);
            row.status = args[args.indexOf("--status") + 1]; row.revision++;
            row.properties = { status: row.status, revision: row.revision };
            // Each CLI command projects all entries, including queued edits.
            const checkbox = { TODO: " ", ACTIVE: "/", PENDING_REVIEW: "r", COMPLETED: "x", CANCELLED: "-" };
            source = document(rows.map((r) => header(r.id, r.status, checkbox[r.status], r.revision)).join("\n"));
            engine.observeInbox(filePath, source);
            return { result: copy(row) };
        },
        notice: (message) => notices.push(message),
    };
    engine = new SyncEngine(io); t.after(() => engine.dispose());
    await engine.initialize();
    return { engine, rows, calls, notices, io, source: () => source,
        replaceSource(value) { source = value; },
        edit() { source = source.replaceAll("- [x]", "- [ ]"); engine.observeInbox(filePath, source); } };
}

test("Inbox retains the file path when a newer checkbox edit is discovered during rollback", async (t) => {
    const f = await inboxFixture(t);
    const read = f.io.read;
    let changed = false;
    f.io.read = async (path, note) => {
        if (!changed && note.id === id && note.status === "TODO") {
            changed = true;
            f.replaceSource(f.source().replace("- [ ] Title", "- [/] Title"));
        }
        return read(path, note);
    };
    f.edit(); await f.engine.flush(); await f.engine.flush();
    assert.equal(f.rows[0].status, "ACTIVE");
    assert.equal(f.rows[1].status, "TODO");
    assert.equal(f.notices.length, 0);
});

test("Inbox edits in the same file queue independently and survive full projections", async (t) => {
    const f = await inboxFixture(t);
    f.edit(); await f.engine.flush();
    assert.equal(f.calls.length, 2);
    assert.deepEqual(f.rows.map((r) => r.status), ["TODO", "TODO"]);
    for (const args of f.calls) {
        assert.deepEqual(args.slice(0, 2), ["inbox", "set-status"]);
        assert.ok(args.includes("--expect-revision"));
        assert.ok(args.includes("--idempotency-key"));
    }
    assert.equal(f.notices.length, 0);
});

test("Inbox failures restore individual entries and notify; startup never replays edits", async (t) => {
    const f = await inboxFixture(t, { fail: true, offline: true });
    assert.equal(f.calls.length, 0);
    assert.equal((f.source().match(/- \[x\]/g) || []).length, 2);
    f.edit(); await f.engine.flush();
    assert.equal(f.calls.length, 2);
    assert.equal((f.source().match(/- \[x\]/g) || []).length, 2);
    assert.match(f.notices[0], /archived/);
});

test("Deleting an Inbox file cancels every queued entry", async (t) => {
    const f = await inboxFixture(t);
    f.edit(); f.engine.forget(filePath); await f.engine.flush();
    assert.equal(f.calls.length, 0);
});
