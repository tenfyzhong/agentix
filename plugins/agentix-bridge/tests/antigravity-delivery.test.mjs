import test from 'node:test';
import assert from 'node:assert/strict';
import { AntigravityDelivery } from '../antigravity/delivery.mjs';

function fixture(overrides = {}) {
    const calls = [];
    const terminal = { inspect: async () => ['42', 'agy', '2', '2', '0', '0'], foreground: async () => {},
        capture: async () => 'Antigravity CLI\n────────────────\n> \n────────────────\n-- INSERT -- ? for shortcuts    Gemini 4 Carbon · medium',
        loadPaste: async (_id, text) => calls.push(['load', text]), pasteLoaded: async () => calls.push(['paste']),
        sendKey: async key => calls.push(['key', key]), clearPaste: async () => calls.push(['clear']), ...overrides };
    const delivery = new AntigravityDelivery({ resolve: async () => terminal, delay: async () => {} });
    return { terminal, delivery, calls };
}
test('antigravity_delivery_verifies_composer_then_pastes_and_submits', async () => {
    const f = fixture(); await f.delivery.send({ text: 'hello\nworld' });
    assert.deepEqual(f.calls, [['load', 'hello\nworld'], ['paste'], ['key', 'Enter'], ['clear']]);
});
test('antigravity_delivery_refuses_drafts_dialogs_normal_mode_and_foreign_processes', async () => {
    for (const overrides of [
        { capture: async () => 'Antigravity CLI\n────────────────\n> draft\n────────────────\n-- INSERT --' },
        { capture: async () => 'Do you trust this folder?\n> Yes\n  No' },
        { capture: async () => 'Antigravity CLI\n────────────────\n> \n────────────────\n-- NORMAL --' },
        { inspect: async () => ['42', 'fish', '2', '2', '0', '0'] },
    ]) {
        const f = fixture(overrides);
        await assert.rejects(f.delivery.send({ text: 'hello' }), error => error.notSent === true);
        assert.deepEqual(f.calls, []);
    }
});
test('antigravity_delivery_rechecks_after_loading_and_distinguishes_uncertain_paste', async () => {
    let inspections = 0;
    const f = fixture({ inspect: async () => (++inspections === 1 ? ['42', 'agy', '2', '2', '0', '0'] : ['42', 'fish', '2', '2', '0', '0']) });
    await assert.rejects(f.delivery.send({ text: 'hello' }), error => error.notSent === true);
    assert.deepEqual(f.calls, [['load', 'hello'], ['clear']]);
    const g = fixture({ pasteLoaded: async () => { throw new Error('timeout'); } });
    await assert.rejects(g.delivery.send({ text: 'hello' }), error => !error.notSent);
});
test('antigravity_stop_uses_escape_and_rejects_an_unverified_foreground', async () => {
    const f = fixture(); await f.delivery.stop();
    assert.deepEqual(f.calls, [['key', 'Escape']]);
    const g = fixture({ foreground: async () => { throw new Error('background'); } });
    await assert.rejects(g.delivery.stop(), /background/);
    assert.deepEqual(g.calls, []);
});

test('antigravity_delivery_waits_for_native_model_initialization', async () => {
    const f = fixture({ capture: async () => 'Antigravity CLI\n────────────────\n> \n────────────────\n-- INSERT -- ? for shortcuts' });
    await assert.rejects(f.delivery.send({ text: 'hello' }), /ready|initializ/i);
    assert.deepEqual(f.calls, []);
});

test('antigravity_delivery_accepts_idle_footer_without_optional_vim_indicator', async () => {
    const f = fixture({ capture: async () => 'Antigravity CLI\n────────────────\n>\n────────────────\n? for shortcuts    Gemini 4 Carbon · medium' });
    await f.delivery.send({ text: 'hello' });
    assert.deepEqual(f.calls, [['load', 'hello'], ['paste'], ['key', 'Enter'], ['clear']]);
});

test('antigravity_draft_clear_requires_matching_contents_and_verifies_empty_composer', async () => {
    let draft = 'local draft';
    const keys = [];
    let redraws = 0;
    const f = fixture({
        inspect: async () => ['42', 'agy', String(2 + draft.length), '2', '0', '0'],
        capture: async () => !draft && redraws++ === 0 ? 'redrawing' : `Antigravity CLI\n────────────────\n> ${draft}\n────────────────\n${draft ? '-- INSERT --' : '? for shortcuts'}    Gemini 4 Carbon · medium`,
        sendKey: async key => { keys.push(key); if (key === 'C-u') draft = ''; },
    });
    assert.equal(await f.delivery.draft(), 'local draft');
    assert.equal(await f.delivery.draft('stale draft'), 'local draft');
    assert.deepEqual(keys, []);
    assert.equal(await f.delivery.draft('local draft'), null);
    assert.deepEqual(keys, ['C-e', 'C-u']);
});
