import assert from "node:assert/strict";
import { test } from "node:test";
import { connectionFixture } from "./support/obsidian-plugin.mjs";

test("native boards load without registering a custom Recent Jobs renderer", async () => {
    const { views } = await connectionFixture();
    assert.deepEqual(views, []);
});

test("board styling follows managed notes and Bases without affecting personal boards", async () => {
    const { plugin, workspaceEvents, metadataEvents } = await connectionFixture();
    plugin.engine = { watches: p => p.startsWith("11-Agents/"), directory: "11-Agents", dispose() {}, observe() {}, observeInbox() {} };
    const leaf = (path, properties) => ({ view: {
        file: { path, properties },
        containerEl: { classList: { toggle(name, on) { this[name] = on; }, remove(name) { this[name] = false; } } },
    } });
    const recent = leaf("11-Agents/Recent Jobs.base");
    const dashboard = leaf("11-Agents/Dashboard.base");
    const personal = leaf("11-Agents/Personal.base");
    const job = leaf("11-Agents/Projects/Demo/Jobs/One.md", { "taskix-generated": true, tags: ["agent/job"] });
    const note = leaf("11-Agents/Personal.md", { tags: ["agent/job"] });
    plugin.app.workspace.getLeavesOfType = type => type === "bases" ? [recent, dashboard, personal] : [job, note];
    plugin.app.metadataCache.getFileCache = file => ({ frontmatter: file.properties });
    workspaceEvents.get("layout-change")();
    for (const item of [recent, dashboard, job]) assert.equal(item.view.containerEl.classList["taskix-board"], true);
    for (const item of [personal, note]) assert.equal(item.view.containerEl.classList["taskix-board"], false);
    recent.view.file.path = "Personal.base";
    workspaceEvents.get("layout-change")();
    assert.equal(recent.view.containerEl.classList["taskix-board"], false, "remove styling when a tab changes files");
    note.view.file.properties = { "taskix-generated": true, tags: ["agent/board"] };
    metadataEvents.get("changed")(note.view.file, "", { frontmatter: note.view.file.properties });
    assert.equal(note.view.containerEl.classList["taskix-board"], true, "style a newly indexed managed note");
    plugin.onunload();
    assert.equal(job.view.containerEl.classList["taskix-board"], false);
});
