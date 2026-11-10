import test from 'node:test';
import assert from 'node:assert/strict';
import { parseModelMenu, parseEffortMenu, AntigravityControls } from '../antigravity/controls.mjs';
const models = selected => `Switch Model\n\n  Search:\n          ────────────────────\n\n${selected === 0 ? '>' : ' '} Gemini 4 Carbon       (current)\n${selected === 1 ? '>' : ' '} Gemini 4 Argon\n\n  Effort  ◂        ●━━━━◉────○        ▸\n                  low medium high\nKeyboard: ↑/↓ Navigate  ←/→ Effort  enter Select  esc Go Back`;
test('native_model_and_effort_menus_preserve_labels_and_selected_index', () => {
    assert.deepEqual(parseModelMenu(models(1)), { choices: ['Gemini 4 Carbon', 'Gemini 4 Argon'], selected: 1 });
    assert.deepEqual(parseEffortMenu('Set Effort\n\n  ◂        ●━━━━━━━━━━━━━━◉──────────────○        ▸\n          low          medium          high\n    Description (current)\nKeyboard: ←/→ Effort  enter Select  esc Go Back'),
        { choices: ['low', 'medium', 'high'], selected: 1 });
    assert.equal(parseModelMenu('Do you trust this folder?\n> Yes\n  No'), null);
    assert.equal(parseEffortMenu('Set Effort\nunknown rendering'), null);
});
function fixture(cached = false) {
    let menu = false, selected = 0, current = 0;
    const keys = [];
    const terminal = { sendText: async text => { assert.equal(text, '/model'); },
        sendKey: async key => { keys.push(key); if (key === 'Enter') { if (menu) { current = selected; menu = false; } else menu = true; }
            if (key === 'Down') selected = 1;
            if (key === 'Escape') menu = false;
        }, capture: async () => menu ? models(selected) : `>\nGemini 4 ${current === 0 ? 'Carbon' : 'Argon'} · medium` };
    const capture = terminal.capture;
    let snapshot;
    if (cached) terminal.capture = async () => snapshot;
    const delivery = { resolve: async () => terminal, check: async () => { assert.equal(menu, false); }, verify: async () => { snapshot = await capture(); } };
    return { controls: new AntigravityControls(delivery, { delay: async () => {} }), keys };
}
test('model_query_closes_menu_and_selection_uses_native_navigation', async () => {
    const f = fixture();
    assert.deepEqual((await f.controls.command('model')).choices, [
        { label: 'Gemini 4 Carbon', value: 'Gemini 4 Carbon' }, { label: 'Gemini 4 Argon', value: 'Gemini 4 Argon' },
    ]);
    assert.deepEqual(f.keys, ['Enter', 'Escape']);
    assert.match((await f.controls.command('model', 'Gemini 4 Argon')).body, /Gemini 4 Argon/);
    assert.deepEqual(f.keys.slice(2), ['Enter', 'Down', 'Enter']);
});
test('unknown_model_is_rejected_without_submitting_a_selection', async () => {
    const f = fixture();
    await assert.rejects(f.controls.command('model', 'unknown'), /not available/);
    assert.deepEqual(f.keys, ['Enter', 'Escape']);
});

test('model_selection_refreshes_cached_terminal_snapshots_after_keys', async () => {
    const f = fixture(true);
    assert.match((await f.controls.command('model', 'Gemini 4 Argon')).body, /Gemini 4 Argon/);
});
