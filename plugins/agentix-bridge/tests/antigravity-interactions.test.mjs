import test from 'node:test';
import assert from 'node:assert/strict';
import { parseInteraction, AntigravityInteractions } from '../antigravity/interactions.mjs';
const approval = (selected = 0, command = '/usr/bin/printf agentix-approval-fixture') => `Earlier conversation\nCommand\n────────────────────────────────────────\n\nRequesting permission for:\n   ${command}\n\nRun this command?\n${selected === 0 ? '>' : ' '} 1. Yes, run command\n${selected === 1 ? '>' : ' '} 2. No, cancel\n\n  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\n-- INSERT -- esc to cancel    Gemini 4 Carbon · medium`;
const fileApproval = (selected = 0, content = 'agentix-file-fixture') => `Create file\n────────────────────────\n\n/tmp/workspace/fixture.txt\n  1  +  ${content}\n\nAllow creation of this file?\n${selected === 0 ? '>' : ' '} 1. Yes, allow creation\n${selected === 1 ? '>' : ' '} 2. No, deny creation\n\n  ↑/↓ Navigate · f full diff\n-- INSERT -- esc to cancel    Gemini 4 Carbon · medium`;
test('native_file_approval_preserves_path_and_diff_and_rejects_stale_content', async () => {
    const parsed = parseInteraction(fileApproval());
    assert.equal(parsed.kind, 'file_approval');
    assert.match(parsed.detail, /fixture.txt\n  1  \+  agentix-file-fixture/);
    assert.equal(parsed.fingerprint, parseInteraction(fileApproval(1)).fingerprint);
    assert.notEqual(parsed.fingerprint, parseInteraction(fileApproval(0, 'different')).fingerprint);
    let selected = 0;
    const keys = [], terminal = { capture: async () => fileApproval(selected), sendKey: async key => { keys.push(key); if (key === 'Down') selected++; } };
    const interactions = new AntigravityInteractions({ resolve: async () => terminal, verify: async () => {} }, { delay: async () => {} });
    await interactions.respond(parsed, { decision: 'No, deny creation' });
    assert.deepEqual(keys, ['Down', 'Enter']);
    assert.equal(parseInteraction(fileApproval() + '\n> new composer'), null);
});
test('native_file_access_approval_includes_operation_and_wrapped_path', () => {
    const screen = 'File access\n────────────────────\n\nRead:\n/tmp/workspace/fixture\n.txt\n\nAllow access to this file?\n> 1. Yes, allow access\n  2. No, deny access\n\n  ↑/↓ Navigate\n-- INSERT -- esc to cancel    Gemini 4 Carbon · medium';
    const parsed = parseInteraction(screen);
    assert.equal(parsed.kind, 'file_approval');
    assert.equal(parsed.title, 'Allow access to this file?');
    assert.equal(parsed.detail, 'Read:\n/tmp/workspace/fixture\n.txt');
});
test('native_pending_edit_preserves_both_sides_of_diff', () => {
    const screen = 'Pending edit\n────────────────────\n\n/tmp/workspace/fixture.txt\n  1  -  previous fixture\n  1  +  agentix-file-fixture\n\n  shift+tab to auto-approve file edits\nAccept this file edit?\n> 1. Yes, accept this change\n  2. No, reject this change\n\n  ↑/↓ Navigate · f full diff\n-- INSERT -- esc to cancel    Gemini 4 Carbon · medium';
    const parsed = parseInteraction(screen);
    assert.equal(parsed.kind, 'file_approval');
    assert.equal(parsed.title, 'Accept this file edit?');
    assert.equal(parsed.detail, '/tmp/workspace/fixture.txt\n  1  -  previous fixture\n  1  +  agentix-file-fixture');
});
test('native_command_approval_identity_excludes_selection_and_preserves_command_scope', () => {
    const first = parseInteraction(approval());
    assert.equal(first.kind, 'command_approval');
    assert.equal(first.detail, '/usr/bin/printf agentix-approval-fixture');
    assert.deepEqual(first.choices, ['Yes, run command', 'No, cancel']);
    assert.equal(first.selected, 0);
    assert.equal(first.fingerprint, parseInteraction(approval(1)).fingerprint);
    assert.notEqual(first.fingerprint, parseInteraction(approval(0, 'other command')).fingerprint);
    assert.equal(parseInteraction(approval() + '\n> new composer'), null);
    assert.equal(parseInteraction('Here is an example:\n> 1. Yes\n  2. No'), null);
});
function fixture({ change = false } = {}) {
    let selected = 0, reads = 0;
    const keys = [];
    const terminal = { capture: async () => approval(selected, change && ++reads > 1 ? 'different command' : undefined),
        sendKey: async key => { keys.push(key); if (key === 'Down') selected++; } };
    const delivery = { resolve: async () => terminal, verify: async () => {} };
    return { interactions: new AntigravityInteractions(delivery, { delay: async () => {} }), keys };
}
test('approval_response_revalidates_scope_and_selection_before_enter', async () => {
    const f = fixture(), expected = await f.interactions.inspect();
    await f.interactions.respond(expected, { decision: 'No, cancel' });
    assert.deepEqual(f.keys, ['Down', 'Enter']);
});
test('stale_or_unknown_approval_cannot_send_input', async () => {
    const f = fixture({ change: true }), expected = await f.interactions.inspect();
    await assert.rejects(f.interactions.respond(expected, { decision: 'Yes, run command' }), /changed/);
    assert.deepEqual(f.keys, []);
    const g = fixture(), current = await g.interactions.inspect();
    await assert.rejects(g.interactions.respond(current, { decision: 'always' }), /Invalid/);
    assert.deepEqual(g.keys, []);
});

const question = ({ multi = false, selected = 0, checked = [], writing = false, draft = '' } = {}) => `Question\n────────────────────────────────────────\n\nQuestion 1/2: Pick features\n\n${selected === 0 ? '>' : ' '} 1. ${multi ? `[${checked.includes(0) ? 'x' : ' '}] ` : ''}Logs\n${selected === 1 ? '>' : ' '} 2. ${multi ? `[${checked.includes(1) ? 'x' : ' '}] ` : ''}Metrics\n${selected === 2 ? '>' : ' '} 3. Write-in...\n\n${writing ? `  Your answer:\n${draft}\n\n  enter Submit · esc Back` : multi ? `  ↑/↓ Navigate · → Next · space Toggle · enter ${selected === 2 ? 'Write-in' : 'Submit'} · esc Skip` : '  ↑/↓ Navigate · → Next · enter Select · esc Skip'}\nesc to cancel  Gemini 4 Carbon · medium`;
test('native_question_parser_tracks_single_multiple_and_write_in_without_changing_identity', () => {
    const single = parseInteraction(question());
    assert.equal(single.kind, 'user_input');
    assert.equal(single.questionId, 'question:1');
    assert.equal(single.multiple, false);
    assert.equal(single.custom, true);
    assert.deepEqual(single.choices, ['Logs', 'Metrics', 'Write-in...']);
    const multi = parseInteraction(question({ multi: true, checked: [1] }));
    assert.equal(multi.multiple, true);
    assert.deepEqual(multi.checked, [1]);
    const writeIn = parseInteraction(question({ multi: true, selected: 2, checked: [1], writing: true, draft: 'local draft' }));
    assert.equal(writeIn.fingerprint, multi.fingerprint);
    assert.equal(writeIn.writing, true);
    assert.equal(writeIn.draft, 'local draft');
});

function questionFixture(options = {}) {
    const state = { multi: false, selected: 0, checked: [], writing: false, draft: '', ...options }, keys = [];
    let buffer;
    const terminal = {
        capture: async () => question(state),
        sendKey: async key => {
            keys.push(key);
            if (key === 'Down') state.selected++;
            if (key === 'Up') state.selected--;
            if (key === 'Space') state.checked = state.checked.includes(state.selected) ? state.checked.filter(index => index !== state.selected) : [...state.checked, state.selected].sort();
            if (key === 'Enter' && state.selected === 2 && !state.writing) state.writing = true;
        },
        loadPaste: async (_id, text) => { buffer = text; },
        pasteLoaded: async () => { state.draft = buffer; keys.push('Paste'); },
        clearPaste: async () => {},
    };
    return { state, keys, interactions: new AntigravityInteractions({ resolve: async () => terminal, verify: async () => {} }, { delay: async () => {} }) };
}
test('single_choice_question_answers_use_native_navigation', async () => {
    const f = questionFixture(), expected = await f.interactions.inspect();
    await f.interactions.respond(expected, { answers: { 'question:1': { answers: ['Metrics'] } } });
    assert.deepEqual(f.keys, ['Down', 'Enter']);
});
test('multiple_choice_and_custom_text_are_submitted_together', async () => {
    const f = questionFixture({ multi: true }), expected = await f.interactions.inspect();
    await f.interactions.respond(expected, { answers: { 'question:1': { answers: ['Metrics', 'Traces'] } } });
    assert.deepEqual(f.state.checked, [1]);
    assert.equal(f.state.draft, 'Traces');
    assert.deepEqual(f.keys, ['Down', 'Space', 'Down', 'Enter', 'Paste', 'Enter']);
});
test('native_question_drafts_and_changed_checkboxes_are_not_overwritten', async () => {
    const f = questionFixture({ multi: true }), expected = await f.interactions.inspect();
    f.state.checked = [0];
    await assert.rejects(f.interactions.respond(expected, { answers: { 'question:1': { answers: ['Metrics'] } } }), /changed/);
    assert.deepEqual(f.keys, []);
    const g = questionFixture({ writing: true, selected: 2, draft: 'local draft' }), draft = await g.interactions.inspect();
    await assert.rejects(g.interactions.respond(draft, { answers: { 'question:1': { answers: ['remote text'] } } }), /draft/);
    assert.deepEqual(g.keys, []);
});
