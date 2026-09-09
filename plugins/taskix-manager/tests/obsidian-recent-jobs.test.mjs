import assert from "node:assert/strict";
import { test } from "node:test";
import { loadPlugin, copy } from "./support/obsidian-plugin.mjs";

test("recent Jobs retain ten sorted entries per status and refresh after transitions", () => {
    const { recentJobsView } = loadPlugin();
    const statuses = ["ACTIVE", "PENDING_REVIEW", "COMPLETED", "CANCELLED"];
    const items = Array.from({ length: 13 }, (_, rank) => statuses.map(status => ({
        path: `${status}/${rank}`, properties: { status },
    }))).flat();
    const view = { dataAdapter: { extractDataItems: () => items } };
    const app = { internalPlugins: { plugins: { bases: { instance: { registrations: {
        tasknotesKanban: { factory: () => view },
    } } } } } };
    assert.equal(recentJobsView(app, {}, {}), view);
    const result = view.dataAdapter.extractDataItems();
    assert.equal(result.length, 40);
    for (const status of statuses) {
        assert.deepEqual(copy(result.filter(item => item.properties.status === status).map(item => item.path)),
            Array.from({ length: 10 }, (_, rank) => `${status}/${rank}`));
    }
    assert.equal(items.length, 52, "must not mutate the shared query data");
    items[0].properties.status = "COMPLETED";
    assert.ok(view.dataAdapter.extractDataItems().some(item => item.path === "ACTIVE/10"));
});

test("recent Jobs report missing TaskNotes and resolve its current factory lazily", () => {
    const { recentJobsView } = loadPlugin();
    assert.throws(() => recentJobsView({}, {}, {}), /TaskNotes/);
});
