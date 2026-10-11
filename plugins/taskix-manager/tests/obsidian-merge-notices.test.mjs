import test from "node:test";
import assert from "node:assert/strict";
import { connectionFixture } from "./support/obsidian-plugin.mjs";

function notice(message) {
    return {
        nodeType: 1, removed: false,
        matches: selector => selector === ".notice",
        closest() { return this; },
        querySelector: () => ({ textContent: message }),
        querySelectorAll: () => [],
        remove() { this.removed = true; },
    };
}

async function setup(t, existing = false) {
    const observers = [];
    class MutationObserver {
        constructor(callback) { this.callback = callback; observers.push(this); }
        observe() {}
        disconnect() { this.disconnected = true; }
    }
    const doc = { body: {}, defaultView: { MutationObserver } };
    const popout = { body: {}, defaultView: { MutationObserver } };
    const leaves = existing ? [{ view: { containerEl: { ownerDocument: popout } } }, { view: { containerEl: { ownerDocument: popout } } }] : [];
    const f = await connectionFixture("11-Agents", { document: doc }, { leaves });
    t.after(() => f.plugin.onunload());
    f.plugin.engine = { watches: path => path.startsWith("11-Agents/"), dispose() {} };
    f.plugin.app.vault.getAbstractFileByPath = path => ({ path });
    f.plugin.app.metadataCache.getFileCache = file => ({ frontmatter: file.path.includes("unmanaged") ? {} : {
        id: "issue_one", "taskix-generated": true,
    } });
    return { ...f, doc, observers };
}

const mergeMessage = path => `“${path}” has been modified externally, merging changes automatically.`;

test("only_native_merge_notices_for_managed_taskix_documents_are_removed", async t => {
    const f = await setup(t);
    assert.equal(f.observers.length, 1);
    const target = notice(mergeMessage("11-Agents/Projects/Demo/Issues/One.md"));
    const ordinary = notice(mergeMessage("Personal.md"));
    const unmanaged = notice(mergeMessage("11-Agents/unmanaged.md"));
    const error = notice("Could not synchronize delivery content: conflict");
    const similar = notice(mergeMessage("11-Agents/Projects/Demo/Issues/One.md") + " Failed to save.");
    f.observers[0].callback([{ addedNodes: [target, ordinary, unmanaged, error, similar] }]);
    assert.equal(target.removed, true);
    for (const entry of [ordinary, unmanaged, error, similar]) assert.equal(entry.removed, false);
});

test("notice_filtering_handles_message_updates_and_disconnects_on_unload", async t => {
    const f = await setup(t);
    assert.equal(f.observers.length, 1);
    const target = notice(mergeMessage("11-Agents/Projects/Demo/Issues/One.md"));
    f.observers[0].callback([{ target: { parentElement: target }, addedNodes: [] }]);
    assert.equal(target.removed, true);
    f.plugin.onunload();
    assert.equal(f.observers[0].disconnected, true);
    const after = notice(mergeMessage("11-Agents/Projects/Demo/Issues/One.md"));
    f.observers[0].callback([{ addedNodes: [after] }]);
    assert.equal(after.removed, false);
});

test("merge_notice_observers_follow_popout_windows_without_duplicates", async t => {
    const f = await setup(t);
    const doc = { body: {}, defaultView: f.doc.defaultView };
    f.workspaceEvents.get("window-open")({}, { document: doc });
    f.workspaceEvents.get("window-open")({}, { document: doc });
    assert.equal(f.observers.length, 2);
    const target = notice(mergeMessage("11-Agents/Projects/Demo/Issues/One.md"));
    const container = { nodeType: 1, matches: () => false, querySelectorAll: () => [target] };
    f.observers[1].callback([{ addedNodes: [container] }]);
    assert.equal(target.removed, true);
    f.workspaceEvents.get("window-close")({}, { document: doc });
    assert.equal(f.observers[1].disconnected, true);
    assert.equal(f.plugin.mergeNoticeObservers.size, 1);
});

test("existing_popouts_are_observed_once_when_plugin_loads", async t => {
    const f = await setup(t, true);
    assert.equal(f.observers.length, 2);
    const target = notice(mergeMessage("11-Agents/Projects/Demo/Issues/One.md"));
    f.observers[1].callback([{ addedNodes: [target] }]);
    assert.equal(target.removed, true);
});
