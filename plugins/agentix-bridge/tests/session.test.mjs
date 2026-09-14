import test from 'node:test';
import assert from 'node:assert/strict';
import { SessionState } from '../session.mjs';
import { hostFixture } from './support.mjs';

test('session state preserves turn identity through history association and restore', () => {
    const host = hostFixture();
    host.entries.push({ id: 'old', type: 'message', message: { role: 'user', content: 'same', timestamp: 1 } });
    const state = new SessionState(host.ctx);
    state.pendingText = 'same'; assert.equal(state.start(), true);
    const id = state.turn.id;
    assert.equal(state.start(), false);
    assert.deepEqual(state.history().turns.map(t => t.id), ['old', id]);
    host.entries.push({ id: 'new', type: 'message', message: { role: 'user', content: 'same', timestamp: 2 } });
    state.associate(); state.finish();
    const restored = new SessionState(host.ctx, state.state());
    assert.deepEqual(restored.history().turns.map(t => t.id), ['old', id]);
    assert.equal(restored.turn.status, 'completed');
});

test('session history pages remain bounded and validate cursors', () => {
    const host = hostFixture();
    for (let i = 0; i < 30; i++) host.entries.push({ id: String(i), type: 'message', message: { role: 'user', content: 'text' } });
    const state = new SessionState(host.ctx);
    assert.equal(state.history().turns.length, 20);
    assert.equal(state.history().older_cursor, '10');
    assert.throws(() => state.history('invalid'), error => error.bridgeCode === 'invalid_request');
});

test('reloading reconciles completed native history with a stale persisted live turn', () => {
    const host = hostFixture();
    const state = new SessionState(host.ctx);
    state.pendingText = 'hello'; state.start();
    host.entries.push({ id: 'user', type: 'message', message: { role: 'user', content: 'hello', timestamp: 1 } });
    state.associate();
    const saved = state.state();
    host.entries.push({ id: 'assistant', type: 'message', message: { role: 'assistant', content: 'finished while disconnected', stopReason: 'stop' } });
    const restored = new SessionState(host.ctx, saved);
    const turn = restored.history().turns.at(-1);
    assert.equal(turn.id, state.turn.id);
    assert.equal(turn.status, 'completed');
    assert.equal(turn.agent_text, 'finished while disconnected');
});

test('restored turn association uses its original user message even after later local turns', () => {
    const host = hostFixture();
    host.entries.push({ id: 'preceding', type: 'message', message: { role: 'user', content: 'old' } });
    const state = new SessionState(host.ctx); state.pendingText = 'queued'; state.start();
    const saved = state.state();
    host.entries.push(
        { id: 'original', type: 'message', message: { role: 'user', content: 'queued' } },
        { id: 'answer', type: 'message', message: { role: 'assistant', content: 'original answer', stopReason: 'stop' } },
        { id: 'later', type: 'message', message: { role: 'user', content: 'later local prompt' } },
    );
    const restored = new SessionState(host.ctx, saved);
    const turns = restored.history().turns;
    assert.deepEqual(turns.map(turn => turn.id), ['preceding', state.turn.id, 'later']);
    assert.equal(turns[1].agent_text, 'original answer');
});

test('session listing does not manufacture new activity timestamps', () => {
    const originalClock = Date.now;
    let now = 1000000;
    Date.now = () => now;
    try {
        const state = new SessionState(hostFixture().ctx);
        const first = state.summary().updatedAt;
        now += 10000;
        assert.equal(state.summary().updatedAt, first);
        state.pendingText = 'new activity'; state.start();
        assert.equal(state.summary().updatedAt, now / 1000);
    } finally { Date.now = originalClock; }
});

test('stable native branches are indexed once and invalidated by new leaves or activity', () => {
    const host = hostFixture();
    let reads = 0;
    host.ctx.sessionManager.getBranch = () => { reads++; return host.entries; };
    host.ctx.sessionManager.getLeafId = () => host.entries.at(-1)?.id;
    host.entries.push({ id: 'a', type: 'message', message: { role: 'user', content: 'first' } });
    const state = new SessionState(host.ctx);
    state.history(); state.history();
    assert.equal(reads, 1);
    host.entries.push({ id: 'b', type: 'message', message: { role: 'user', content: 'second' } });
    assert.deepEqual(state.history().turns.map(t => t.id), ['a', 'b']);
    host.entries[1].message = { role: 'user', content: 'updated' }; state.touch();
    assert.equal(state.history().turns.at(-1).user_text, 'updated');
});

test('native history restores reasoning and tool input/output with stable identities', () => {
    const host = hostFixture();
    host.entries.push(
        { id: 'u', type: 'message', message: { role: 'user', content: 'check' } },
        { id: 'a', type: 'message', message: { role: 'assistant', content: [
            { type: 'thinking', thinking: 'Consider options' },
            { type: 'toolCall', id: 'call', name: 'bash', arguments: { command: 'pwd' } },
        ] } },
        { id: 't', type: 'message', message: { role: 'toolResult', toolCallId: 'call', toolName: 'bash', content: [{ type: 'text', text: '/work' }], isError: false } },
        { id: 'b', type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: 'Done' }] } },
    );
    const state = new SessionState(host.ctx);
    const turn = state.history().turns[0];
    assert.deepEqual(turn.items, [
        { id: 'u:reasoning:0', kind: 'reasoning', text: 'Consider options', status: 'completed' },
        { id: 'call', kind: 'bash', text: '{"command":"pwd"}\n\n/work', status: 'completed' },
    ]);
    assert.equal(turn.agent_text, 'Done');
    assert.deepEqual(state.history().turns[0].items, turn.items);
    const restored = new SessionState(host.ctx, { turn: { ...turn, user_entry_id: 'u' }, history_ids: [['u', 'u']] });
    assert.deepEqual(restored.history().turns[0].items, turn.items);
});

test('native process history keeps mapped turn IDs and bounds long histories after reconstruction', () => {
    const host = hostFixture();
    host.entries.push({ id: 'u', type: 'message', message: { role: 'user', content: 'check' } });
    for (let i = 0; i < 12; i++) {
        host.entries.push({ id: `a${i}`, type: 'message', message: { role: 'assistant', content: [
            { type: 'thinking', thinking: `Thought ${i}` },
            { type: 'toolCall', id: `call${i}`, name: 'read', arguments: { path: `${i}` } },
        ] } });
        host.entries.push({ id: `t${i}`, type: 'message', message: { role: 'toolResult', toolCallId: `call${i}`, toolName: 'read', content: [{ type: 'text', text: `Result ${i}` }], isError: i === 11 } });
    }
    const state = new SessionState(host.ctx, { history_ids: [['u', 'mapped']] });
    const turn = state.history().turns[0];
    assert.equal(turn.id, 'mapped');
    assert.equal(turn.items.length, 20);
    assert.equal(turn.items[0].id, 'mapped:reasoning:4');
    assert.equal(turn.items.at(-1).id, 'call11');
    assert.equal(turn.items.at(-1).status, 'failed');
    assert.match(turn.items.at(-1).text, /Result 11/);
    assert.equal(turn.tools.length, 12);
});

test('native history retains an orphan tool result without inventing a second item', () => {
    const host = hostFixture();
    host.entries.push(
        { id: 'u', type: 'message', message: { role: 'user', content: 'check' } },
        { id: 't', type: 'message', message: { role: 'toolResult', toolCallId: 'orphan', toolName: 'read', content: 'Missing file', isError: true } },
    );
    const turn = new SessionState(host.ctx).history().turns[0];
    assert.deepEqual(turn.items, [{ id: 'orphan', kind: 'read', text: 'Missing file', status: 'failed' }]);
    assert.equal(turn.tools.length, 1);
});
