import test from "node:test";
import assert from "node:assert/strict";
import { connectionFixture } from "./support/obsidian-plugin.mjs";

test("project merge menu uses authoritative matching remotes and both revisions", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true, reconcile: async () => {} };
    let offered;
    f.plugin.openProjectMergeDialog = (project, candidates) => { offered = { project, candidates }; };
    const action = f.plugin.prepareProjectMerge("prj_demo", "11-Agents/Projects/demo/Board.md");
    f.reply(null, undefined, { id: "prj_demo", key: "demo", remote: "origin", revision: 3 });
    await new Promise(resolve => setImmediate(resolve));
    f.reply(null, undefined, [{ id: "prj_other", key: "demo-2", remote: "origin", revision: 4 }, { id: "prj_foreign", remote: "different" }]);
    await new Promise(resolve => setImmediate(resolve));
    f.reply(null, undefined, []);
    await action;
    assert.equal(offered.candidates.length, 1);
    const merge = f.plugin.mergeProject(offered.project, offered.candidates[0]);
    assert.deepEqual(Array.from(f.requests.at(-1).args.slice(-10,-1)), ["project", "merge", "prj_demo", "prj_other", "--second-revision", "4", "--expect-revision", "3", "--idempotency-key"]);
    f.reply(null, undefined, { id: "prj_demo", retained_id: "prj_demo", removed_id: "prj_other", root: "/new/repo", document_directory: "Projects/demo" });
    await merge;
    assert.match(f.notices.at(-1).message, /merged/);
    for (const field of ["prj_demo", "prj_other", "/new/repo", "Projects/demo"]) assert.ok(f.notices.at(-1).message.includes(field));
});

test("copied project board cannot offer a merge", async () => {
    const f = await connectionFixture();
    f.plugin.engine = { directory: "11-Agents", watches: () => true };
    const action = f.plugin.prepareProjectMerge("prj_demo", "11-Agents/Projects/copy/Board.md");
    f.reply(null, undefined, { id: "prj_demo", key: "demo", remote: "origin", revision: 1 });
    await action;
    assert.equal(f.requests.length, 1);
    assert.match(f.notices.at(-1).message, /registered Project/);
});
