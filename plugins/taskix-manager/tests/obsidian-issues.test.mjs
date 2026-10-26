import test from "node:test";
import assert from "node:assert/strict";
import { connectionFixture, loadPlugin, fixture, copy } from "./support/obsidian-plugin.mjs";

test("removed Inbox entries have no plugin parser or status compatibility", async t => {
    const plugin = loadPlugin();
    assert.equal(plugin.parseInbox, undefined);
    assert.equal(plugin.patchInbox, undefined);
    assert.throws(() => plugin.commandFor({kind:"inbox",id:"inbox_old",status:"TODO"},"ACTIVE"));
    const f = await fixture(); t.after(() => f.engine.dispose());
    f.engine.observe(f.row.path,{id:"inbox_old",status:"ACTIVE",revision:1});
    await f.engine.flush();
    assert.equal(f.calls.length,0);
});

test("Issue milestone edits use revision guards and restore rejected cross-project links", async t => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    Object.assign(f.row, { kind: "issue", id: "issue_one", status: "TODO" });
    f.row.properties.milestone_id = null;
    Object.assign(f.files.get(f.row.path), { id: "issue_one", milestone_id: null });
    await f.engine.initialize();
    const file = f.files.get(f.row.path);
    file.milestone_id = "milestone_elsewhere";
    f.io.execute = async args => { f.calls.push(copy(args)); throw new Error("Milestone belongs to another Project"); };
    f.engine.observe(f.row.path, copy(file));
    await f.engine.flush();
    assert.equal(f.calls[0]?.[1], "update");
    assert.ok(f.calls[0].includes("--expect-revision"));
    assert.ok(f.calls[0].includes("milestone_elsewhere"));
    assert.equal(file.milestone_id, null);
    assert.match(f.notices.at(-1), /another Project/);
});

test("delivery authoring synchronizes one note and ignores identical projection echoes", async t => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    f.plugin.engine = { watches: () => true, flush: async () => {}, disposed: false, dispose() {} };
    const source = ["requirement", "acceptance", "notes"].map(key => `<!-- taskix:${key}:start -->\nAuthored ${key}\n<!-- taskix:${key}:end -->`).join("\n");
    const props = { id:"issue_one", "taskix-generated":true };
    const pending = f.plugin.syncDelivery("11-Agents/Projects/Demo/Issues/One.md", source, props);
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(Array.from(f.requests.at(-1).args.slice(-3)), ["obsidian", "sync", "issue_one"]);
    f.reply(null, undefined, { id:"issue_one" }); await pending;
    await f.plugin.syncDelivery("11-Agents/Projects/Demo/Issues/One.md", source, props);
    assert.equal(f.requests.length, 1);
    await f.plugin.syncDelivery("11-Agents/Projects/Demo/Issues/One.md", source.replace("<!-- taskix:notes:end -->", ""), props);
    assert.equal(f.requests.length, 1);
    assert.match(f.notices.at(-1).message, /markers/);
});

test("moving an Issue to a Milestone link resolves only the same Project and commits its ID", async t => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    Object.assign(f.row, {kind:"issue",id:"issue_one",status:"TODO"});
    Object.assign(f.row.properties, {milestone_id:null,milestone:null});
    Object.assign(f.files.get(f.row.path), {id:"issue_one",milestone_id:null,milestone:null});
    await f.engine.initialize();
    f.io.execute = async args => {
        f.calls.push(copy(args));
        if (args[0] === "milestone") return {result:[{id:"milestone_mvp",project_id:"prj_one",document_path:"Projects/Demo/Milestones/MVP.md"}]};
        assert.ok(args.includes("milestone_mvp"));
        Object.assign(f.row.properties,{milestone_id:"milestone_mvp",milestone:"[[Tasks/Projects/Demo/Milestones/MVP]]"});
        f.row.revision++;
        return {result:{...copy(f.row),milestone_id:"milestone_mvp"}};
    };
    const file = f.files.get(f.row.path);
    file.milestone = "[[Tasks/Projects/Demo/Milestones/MVP]]";
    f.engine.observe(f.row.path,copy(file)); await f.engine.flush();
    assert.equal(f.calls.length,2);
    assert.ok(f.calls[0].includes("prj_one"));
    assert.equal(file.milestone_id,"milestone_mvp");
    assert.equal(f.notices.length,0);
});

test("an unregistered Milestone link is restored and reports the rejection", async t => {
    const f = await fixture(); t.after(() => f.engine.dispose());
    Object.assign(f.row,{kind:"issue",id:"issue_one",status:"TODO"});
    Object.assign(f.row.properties,{milestone_id:null,milestone:null});
    Object.assign(f.files.get(f.row.path),{id:"issue_one",milestone_id:null,milestone:null});
    await f.engine.initialize();
    f.io.execute = async args => { f.calls.push(copy(args)); return {result:[]}; };
    const file=f.files.get(f.row.path);
    file.milestone="[[Missing]]";
    f.engine.observe(f.row.path,copy(file)); await f.engine.flush();
    assert.equal(file.milestone,null);
    assert.equal(f.calls.length,1);
    assert.match(f.notices.at(-1), /Milestone link is missing/);
});

test("Issue status edits use human acceptance transitions without changing Task commands", () => {
    const { commandFor } = loadPlugin();
    assert.deepEqual(Array.from(commandFor({ kind: "issue", id: "issue_one", status: "TODO" }, "ACTIVE")), ["issue", "start", "issue_one"]);
    assert.deepEqual(Array.from(commandFor({ kind: "issue", id: "issue_one", status: "ACTIVE" }, "PENDING_REVIEW")), ["issue", "submit", "issue_one"]);
    assert.deepEqual(Array.from(commandFor({ kind: "issue", id: "issue_one", status: "PENDING_REVIEW" }, "COMPLETED")), ["issue", "approve", "issue_one"]);
    assert.equal(commandFor({ kind: "issue", id: "issue_one", status: "COMPLETED" }, "TODO")[1], "reopen");
    assert.throws(() => commandFor({ kind: "issue", id: "issue_one", status: "ACTIVE" }, "COMPLETED"));
    assert.throws(() => commandFor({ kind: "task", id: "task_one", status: "TODO" }, "IN_PROGRESS"), /leases/);
});

test("direct managed Issues folder exposes New Issue and reads registered Milestones", async () => {
    const f = await connectionFixture();
    const folder = { path: "11-Agents/Projects/Demo/Issues", children: [] };
    const board = { path: "11-Agents/Projects/Demo/Board.md" };
    f.plugin.engine = { directory: "11-Agents", watches: () => true };
    f.plugin.app.vault.getAbstractFileByPath = candidate => candidate === board.path ? board : null;
    f.plugin.app.metadataCache.getFileCache = () => ({ frontmatter: { id: "prj_demo", status: "ACTIVE", "taskix-generated": true } });
    let dialog;
    f.plugin.openIssueDialog = (project, folderPath, milestones) => { dialog = { project, folderPath, milestones }; };
    const items = [];
    const menu = { addItem(callback) {
        const item = { setTitle(title) { this.title = title; return this; }, setIcon() { return this; }, onClick(click) { this.click = click; return this; } };
        callback(item); items.push(item);
    } };
    f.workspaceEvents.get("file-menu")(menu, folder);
    assert.equal(items[0]?.title, "New Issue");
    const pending = items[0].click();
    f.reply(null, undefined, { id: "prj_demo", key: "Demo", revision: 3, archived_at: null });
    await new Promise(resolve => setImmediate(resolve));
    assert.ok(f.requests.at(-1).args.includes("milestone"));
    f.reply(null, undefined, [{ id: "milestone_one", title: "MVP" }]);
    await pending;
    assert.equal(dialog.folderPath, folder.path);
    assert.equal(dialog.milestones[0].id, "milestone_one");
    f.workspaceEvents.get("file-menu")({ addItem() { assert.fail("nested folder"); } }, { path: `${folder.path}/nested`, children: [] });
});

test("copied or archived Issues folder cannot create a requirement", async () => {
    for (const [project, folderPath] of [
        [{ id: "prj_demo", key: "Demo", revision: 3, archived_at: null }, "11-Agents/Projects/Copy/Issues"],
        [{ id: "prj_demo", key: "Demo", revision: 3, archived_at: 1 }, "11-Agents/Projects/Demo/Issues"],
    ]) {
        const f = await connectionFixture();
        f.plugin.engine = { directory: "11-Agents", watches: () => true };
        f.plugin.openIssueDialog = () => assert.fail("invalid folder");
        const pending = f.plugin.prepareIssue("prj_demo", folderPath);
        f.reply(null, undefined, project);
        await pending;
        assert.equal(f.requests.length, 1);
        assert.match(f.notices.at(-1).message, /registered active Project/);
    }
});

test("Issue creation invokes the shared CLI template and opens the returned note", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true };
    const file = { path: "11-Agents/Projects/Demo/Issues/261010-0001-Login.md" };
    f.plugin.app.vault.getAbstractFileByPath = candidate => candidate === file.path ? file : null;
    let opened;
    f.plugin.app.workspace.getLeaf = () => ({ openFile: async file => { opened = file; } });
    const pending = f.plugin.createIssue({ id: "prj_demo", key: "Demo" }, "11-Agents/Projects/Demo/Issues", { title: "Login", milestone: "milestone_one" }, "create-once");
    assert.ok(f.requests.at(-1).args.includes("issue"));
    assert.ok(f.requests.at(-1).args.includes("--milestone"));
    assert.ok(f.requests.at(-1).args.includes("--idempotency-key"));
    assert.ok(f.requests.at(-1).args.includes("create-once"));
    f.reply(null, undefined, { id: "issue_one", project_id: "prj_demo", document_path: "Projects/Demo/Issues/261010-0001-Login.md" });
    await pending;
    assert.equal(opened, file);
});

test("Issue creation does not create a blank note when its projection is not cached", async t => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    f.plugin.engine = { directory:"11-Agents", watches:()=>true, dispose() {} };
    let linked = false;
    f.plugin.app.workspace.openLinkText = async () => { linked = true; };
    const pending = f.plugin.createIssue({id:"prj_demo", key:"Demo"}, "11-Agents/Projects/Demo/Issues", {title:"Login"}, "one");
    f.reply(null, undefined, {id:"issue_one",project_id:"prj_demo",document_path:"Projects/Demo/Issues/One.md"});
    await pending;
    assert.equal(linked, false);
    assert.match(f.notices.at(-1).message, /created.*Refresh/);
});

for (const kind of ["issue", "milestone"]) {
    test(`${kind}_editing_waits_five_idle_seconds_and_cancels_on_unload`, async t => {
        t.mock.timers.enable({ apis: ["setTimeout"] });
        const f = await connectionFixture();
        t.after(() => f.plugin.onunload());
        const calls = [];
        f.plugin.syncDelivery = async (...args) => calls.push(args);
        const filePath = `11-Agents/Projects/Demo/${kind}.md`;
        const properties = { id: `${kind}_one`, "taskix-generated": true };
        const flush = async () => { await f.plugin.deliveryQueue; };
        f.plugin.queueDelivery(filePath, "first edit", properties);
        t.mock.timers.tick(4999); await flush();
        assert.equal(calls.length, 0);
        f.plugin.queueDelivery(filePath, "latest edit", properties);
        t.mock.timers.tick(4999); await flush();
        assert.equal(calls.length, 0);
        t.mock.timers.tick(1); await flush();
        assert.equal(calls.length, 1);
        assert.equal(calls[0][1], "latest edit");
        f.plugin.queueDelivery(filePath, "cancelled edit", properties);
        f.plugin.onunload();
        t.mock.timers.tick(5000); await flush();
        assert.equal(calls.length, 1);
    });
}

for (const kind of ["issue", "milestone"]) {
    test(`${kind}_pending_delivery_survives_unload_and_recovers_closed_note`, async t => {
        t.mock.timers.enable({ apis: ["setTimeout"] });
        const storage = new Map();
        const first = await connectionFixture("11-Agents", {}, { storage });
        const filePath = `11-Agents/Projects/Demo/${kind}.md`;
        const props = { id: `${kind}_one`, "taskix-generated": true };
        const keys = kind === "issue" ? ["requirement", "acceptance", "notes"] : ["goal"];
        const body = keys.map(key => `<!-- taskix:${key}:start -->\nSaved content\n<!-- taskix:${key}:end -->`).join("\n");
        first.plugin.queueDelivery(filePath, body, props);
        first.plugin.onunload();
        const second = await connectionFixture("11-Agents", {}, { storage });
        t.after(() => second.plugin.onunload());
        second.file.path = filePath;
        second.plugin.app.vault.getAbstractFileByPath = path => path === filePath ? second.file : null;
        second.plugin.app.vault.read = async () => `---\n${JSON.stringify(props)}\n---\n${body}`;
        second.plugin.engine = { directory: "11-Agents", watches: () => true, observe() {}, flush: async () => {}, dispose() {} };
        await second.plugin.recoverDeliveries(second.plugin.engine);
        t.mock.timers.tick(5000);
        await new Promise(resolve => setImmediate(resolve));
        assert.equal(second.requests.length, 1);
        second.reply(null, undefined, { id: props.id });
        await second.plugin.deliveryQueue;
        assert.deepEqual([...second.plugin.pendingDeliveries], []);
        const third = await connectionFixture("11-Agents", {}, { storage });
        t.after(() => third.plugin.onunload());
        assert.equal(third.plugin.pendingDeliveries.size, 0);
    });
}

test("title-only Issue edits trigger delivery after an unchanged body was synchronized", async t => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    f.plugin.engine = { watches: () => true, flush: async () => {}, disposed: false, dispose() {} };
    const source = ["requirement", "acceptance", "notes"].map(key => `<!-- taskix:${key}:start -->\nAuthored ${key}\n<!-- taskix:${key}:end -->`).join("\n");
    const props = { id: "issue_one", title: "Old title", "taskix-generated": true };
    const path = "11-Agents/Projects/Demo/Issues/One.md";
    let pending = f.plugin.syncDelivery(path, source, props);
    await new Promise(resolve => setImmediate(resolve));
    f.reply(null, undefined, { id: "issue_one" }); await pending;
    pending = f.plugin.syncDelivery(path, source, { ...props, title: "New title" });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(f.requests.length, 2);
    f.reply(null, undefined, { id: "issue_one" }); await pending;
});

test("renaming an Issue note queues delivery under the new path with its stable ID", async t => {
    const f = await connectionFixture(); t.after(() => f.plugin.onunload());
    const forgotten = [], deliveries = [];
    f.plugin.engine = { directory: "11-Agents", watches: () => true, disposed: false,
        dispose() {}, forget: path => forgotten.push(path), observe() {} };
    const props = { id: "issue_one", title: "Old title", "taskix-generated": true };
    const source = `---\n${JSON.stringify(props)}\n---\n# Old title\n`;
    f.plugin.app.vault.read = async () => source;
    f.plugin.queueDelivery = (...args) => deliveries.push(args);
    const oldPath = "11-Agents/Projects/Demo/Issues/261026-0001-Old title.md";
    f.file.path = "11-Agents/Projects/Demo/Issues/261026-0001-New title.md";
    await f.vaultEvents.get("rename")(f.file, oldPath);
    assert.deepEqual(forgotten, [oldPath]);
    assert.equal(deliveries.length, 1);
    assert.equal(deliveries[0][0], f.file.path);
    assert.equal(deliveries[0][2].id, "issue_one");
});
