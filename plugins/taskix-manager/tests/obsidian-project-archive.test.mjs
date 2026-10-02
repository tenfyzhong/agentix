import test from "node:test";
import assert from "node:assert/strict";
import { connectionFixture } from "./support/obsidian-plugin.mjs";

test("project context menu archives a managed Board using the authoritative revision", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true, forget() {}, reconcile: async () => {} };
    const file = { path: "11-Agents/Projects/Demo/Board.md" };
    f.plugin.app.metadataCache.getFileCache = () => ({ frontmatter: { id: "prj_demo", revision: 2, status: "ACTIVE", "taskix-generated": true, tags: ["agent/project"] } });
    const items = [];
    const menu = { addItem(callback) {
        const item = { setTitle(title) { this.title = title; return this; }, setIcon() { return this; }, onClick(callback) { this.click = callback; return this; } };
        callback(item); items.push(item);
    } };
    const handler = f.workspaceEvents.get("file-menu");
    assert.equal(typeof handler, "function");
    handler(menu, file);
    assert.equal(items[0].title, "Archive project");
    const action = items[0].click();
    assert.ok(f.requests.at(-1).args.includes("show"));
    f.reply(null, undefined, { id: "prj_demo", key: "Demo", revision: 5, archived_at: null });
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(Array.from(f.requests.at(-1).args.slice(-7, -1)), ["project", "archive", "prj_demo", "--expect-revision", "5", "--idempotency-key"]);
    f.reply(null, undefined, { id: "prj_demo", revision: 6, archived_at: 1 });
    await action;
    assert.ok(f.notices.some(n => n.message.includes("archived")));
});

test("copied Board cannot archive a different registered project folder", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true };
    const action = f.plugin.archiveProject("prj_demo", true, "11-Agents/Projects/Copy/Board.md");
    f.reply(null, undefined, { id: "prj_demo", key: "Demo", revision: 5, archived_at: null });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(f.requests.length, 1);
    await action;
    assert.match(f.notices.at(-1).message, /registered Project/);
});

test("folder menu restores an archived project and reports a pending move", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true, forget() {}, reconcile: async () => {} };
    const folder = { path: "11-Agents/History/Projects/Demo", children: [] };
    const board = { path: `${folder.path}/Board.md` };
    f.plugin.app.vault.getAbstractFileByPath = () => board;
    f.plugin.app.metadataCache.getFileCache = () => ({ frontmatter: { id: "prj_demo", status: "ARCHIVED", "taskix-generated": true } });
    let item;
    f.workspaceEvents.get("file-menu")({ addItem(callback) {
        item = { setTitle(title) { this.title = title; return this; }, setIcon() { return this; }, onClick(callback) { this.click = callback; return this; } };
        callback(item);
    } }, folder);
    assert.equal(item.title, "Restore project");
    const action = item.click();
    f.reply(null, undefined, { id: "prj_demo", key: "Demo", document_directory: "History/Projects/Demo", revision: 7, archived_at: 1 });
    await new Promise(resolve => setImmediate(resolve));
    assert.ok(f.requests.at(-1).args.includes("unarchive"));
    f.requests.at(-1).callback(null, JSON.stringify({ schema_version: 1, ok: true, result: {}, projection_pending: "destination exists" }), "");
    await action;
    assert.match(f.notices.at(-1).message, /document move is pending/);
});

test("archive guards surface CLI conflicts and exclude unmanaged notes", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true };
    f.plugin.app.metadataCache.getFileCache = () => ({ frontmatter: { id: "prj_demo", status: "ACTIVE" } });
    f.workspaceEvents.get("file-menu")({ addItem() { assert.fail("unmanaged Board"); } }, { path: "11-Agents/Projects/Demo/Board.md" });
    const action = f.plugin.archiveProject("prj_demo", true, "11-Agents/Projects/Demo/Board.md");
    f.reply(null, undefined, { id: "prj_demo", key: "Demo", revision: 8, archived_at: null });
    await new Promise(resolve => setImmediate(resolve));
    f.reply("complete or cancel all Jobs before archiving Project");
    await action;
    assert.match(f.notices.at(-1).message, /complete or cancel all Jobs/);
});
