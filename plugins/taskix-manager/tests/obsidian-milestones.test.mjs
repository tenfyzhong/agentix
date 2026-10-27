import test from "node:test";
import assert from "node:assert/strict";
import { connectionFixture, loadPlugin, fixture, copy } from "./support/obsidian-plugin.mjs";

test("direct managed Milestones folder exposes New Milestone", async () => {
    const f = await connectionFixture();
    const folder = { path: "11-Agents/Projects/Demo/Milestones", children: [] };
    const board = { path: "11-Agents/Projects/Demo/Board.md" };
    f.plugin.engine = { directory: "11-Agents", watches: () => true };
    f.plugin.app.vault.getAbstractFileByPath = candidate => candidate === board.path ? board : null;
    f.plugin.app.metadataCache.getFileCache = () => ({ frontmatter: { id: "prj_demo", status: "ACTIVE", "taskix-generated": true } });
    let dialog;
    f.plugin.openMilestoneDialog = (project, folderPath) => { dialog = { project, folderPath }; };
    const items = [];
    const menu = { addItem(callback) {
        const item = { setTitle(title) { this.title = title; return this; }, setIcon() { return this; }, onClick(click) { this.click = click; return this; } };
        callback(item); items.push(item);
    } };
    f.workspaceEvents.get("file-menu")(menu, folder);
    assert.equal(items[0]?.title, "New Milestone");
    const pending = items[0].click();
    f.reply(null, undefined, { id: "prj_demo", key: "Demo", revision: 3, archived_at: null });
    await pending;
    assert.equal(dialog.folderPath, folder.path);
    f.workspaceEvents.get("file-menu")({ addItem() { assert.fail("nested folder"); } }, { path: `${folder.path}/nested`, children: [] });
});

test("copied or archived Milestones folder cannot create a milestone", async () => {
    for (const [project, folderPath] of [
        [{ id: "prj_demo", key: "Demo", revision: 3, archived_at: null }, "11-Agents/Projects/Copy/Milestones"],
        [{ id: "prj_demo", key: "Demo", revision: 3, archived_at: 1 }, "11-Agents/Projects/Demo/Milestones"],
    ]) {
        const f = await connectionFixture();
        f.plugin.engine = { directory: "11-Agents", watches: () => true };
        f.plugin.openMilestoneDialog = () => assert.fail("invalid folder");
        const pending = f.plugin.prepareMilestone("prj_demo", folderPath);
        f.reply(null, undefined, project);
        await pending;
        assert.equal(f.requests.length, 1);
        assert.match(f.notices.at(-1).message, /registered active Project/);
    }
});

test("Milestone creation invokes the shared CLI template and opens the returned note", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true };
    const file = { path: "11-Agents/Projects/Demo/Milestones/261010-0001-Login.md" };
    f.plugin.app.vault.getAbstractFileByPath = candidate => candidate === file.path ? file : null;
    let opened;
    f.plugin.app.workspace.getLeaf = () => ({ openFile: async file => { opened = file; } });
    const pending = f.plugin.createMilestone({ id: "prj_demo", key: "Demo" }, "11-Agents/Projects/Demo/Milestones", { title: "Login", goal: "Deliver login" }, "create-once");
    assert.ok(f.requests.at(-1).args.includes("milestone"));
    assert.ok(f.requests.at(-1).args.includes("--goal"));
    assert.ok(f.requests.at(-1).args.includes("--idempotency-key"));
    assert.ok(f.requests.at(-1).args.includes("create-once"));
    f.reply(null, undefined, { id: "milestone_one", project_id: "prj_demo", document_path: "Projects/Demo/Milestones/261010-0001-Login.md" });
    await pending;
    assert.equal(opened, file);
});

test("Milestone creation does not create a blank note when its projection is not cached", async t => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    f.plugin.engine = { directory:"11-Agents", watches:()=>true, dispose() {} };
    let linked = false;
    f.plugin.app.workspace.openLinkText = async () => { linked = true; };
    const pending = f.plugin.createMilestone({id:"prj_demo", key:"Demo"}, "11-Agents/Projects/Demo/Milestones", {title:"Login"}, "one");
    f.reply(null, undefined, {id:"milestone_one",project_id:"prj_demo",document_path:"Projects/Demo/Milestones/One.md"});
    await pending;
    assert.equal(linked, false);
    assert.match(f.notices.at(-1).message, /created.*Refresh/);
});

test("Milestone creation rejects blank titles and foreign returned documents", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true };
    const project = { id: "prj_demo", key: "Demo" };
    const folder = "11-Agents/Projects/Demo/Milestones";
    await assert.rejects(f.plugin.createMilestone(project, folder, { title: "  " }, "empty"), /title is required/);
    assert.equal(f.requests.length, 0);
    const pending = f.plugin.createMilestone(project, folder, { title: "MVP" }, "foreign");
    f.reply(null, undefined, { id: "milestone_one", project_id: "prj_other", document_path: "Projects/Other/Milestones/MVP.md" });
    await assert.rejects(pending, /does not match this Project/);
});

test("Milestone status edits map to explicit lifecycle commands", () => {
    const { commandFor } = loadPlugin();
    for (const [from, to, command] of [["PLANNED", "ACTIVE", "start"], ["PLANNED", "COMPLETED", "complete"], ["ACTIVE", "COMPLETED", "complete"], ["ACTIVE", "CANCELLED", "cancel"], ["COMPLETED", "PLANNED", "reopen"], ["CANCELLED", "PLANNED", "reopen"]]) {
        assert.deepEqual(Array.from(commandFor({kind: "milestone", id: "milestone_one", status: from}, to)), ["milestone", command, "milestone_one"]);
    }
    assert.throws(() => commandFor({kind: "milestone", id: "milestone_one", status: "COMPLETED"}, "ACTIVE"));
});

test("Milestone status edit restores rejected completion with revision guards", async t => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    Object.assign(f.row, {kind: "milestone", id: "milestone_one", status: "ACTIVE"});
    Object.assign(f.row.properties, {status: "ACTIVE"});
    Object.assign(f.files.get(f.row.path), {id: "milestone_one", status: "ACTIVE"});
    await f.engine.initialize();
    f.io.execute = async args => { f.calls.push(copy(args)); throw new Error("Milestone has unfinished Issues"); };
    const file = f.files.get(f.row.path);
    file.status = "COMPLETED";
    f.engine.observe(f.row.path, copy(file)); await f.engine.flush();
    assert.equal(f.calls[0]?.[1], "complete");
    assert.ok(f.calls[0].includes("--expect-revision"));
    assert.equal(file.status, "ACTIVE");
    assert.match(f.notices.at(-1), /unfinished Issues/);
});

test("title-only Milestone edits trigger delivery after an unchanged body was synchronized", async t => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    f.plugin.engine = { watches: () => true, flush: async () => {}, disposed: false, dispose() {} };
    const source = ["goal"].map(key => `<!-- taskix:${key}:start -->\nAuthored ${key}\n<!-- taskix:${key}:end -->`).join("\n");
    const props = { id: "milestone_one", title: "Old title", "taskix-generated": true };
    const path = "11-Agents/Projects/Demo/Milestones/One.md";
    let pending = f.plugin.syncDelivery(path, source, props);
    await new Promise(resolve => setImmediate(resolve));
    f.reply(null, undefined, { id: "milestone_one" }); await pending;
    pending = f.plugin.syncDelivery(path, source, { ...props, title: "New title" });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(f.requests.length, 2);
    f.reply(null, undefined, { id: "milestone_one" }); await pending;
});

test("renaming a Milestone note queues delivery under the new path with its stable ID", async t => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    const forgotten = [], deliveries = [];
    f.plugin.engine = { directory: "11-Agents", watches: () => true, disposed: false,
        dispose() {}, forget: path => forgotten.push(path), observe() {} };
    const props = { id: "milestone_one", title: "Old title", "taskix-generated": true };
    const source = `---\n${JSON.stringify(props)}\n---\n# Old title\n`;
    f.plugin.app.vault.read = async () => source;
    f.plugin.queueDelivery = (...args) => deliveries.push(args);
    const oldPath = "11-Agents/Projects/Demo/Milestones/261026-0001-Old title.md";
    f.file.path = "11-Agents/Projects/Demo/Milestones/261026-0001-New title.md";
    await f.vaultEvents.get("rename")(f.file, oldPath);
    assert.deepEqual(forgotten, [oldPath]);
    assert.equal(deliveries.length, 1);
    assert.equal(deliveries[0][0], f.file.path);
    assert.equal(deliveries[0][2].id, "milestone_one");
});
