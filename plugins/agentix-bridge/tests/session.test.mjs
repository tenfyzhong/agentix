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
