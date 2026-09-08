import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { ClaudeStateStore } from '../claude/store.mjs';
import { ClaudeSession } from '../claude/session.mjs';

const identity = { hook_event_name: 'SessionStart', session_id: 's', cwd: '/tmp', transcript_path: '/tmp/s.jsonl' };
function storeFor(t) {
    const root = mkdtempSync(join(tmpdir(), 'ax-store-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    return join(root, 'session.json');
}
test('Claude journal restores legacy turns, receipts and incremental changes', t => {
    const path = storeFor(t);
    writeFileSync(path, JSON.stringify({ turns: [{ id: 'old', status: 'completed' }], receipts: [] }));
    const store = new ClaudeStateStore(path);
    store.load(() => { throw new Error('Legacy state must be preserved'); });
    store.append({ turn: { id: 'new', status: 'inProgress' }, receipts: [['request', { state: 'pending', text: 'hello', turn_id: 'new' }]] });
    store.append({ turn: { id: 'new', status: 'completed', agent_text: 'answer' }, receipts: [['request', { state: 'accepted', text: 'hello', turn_id: 'new' }]] });
    store.append({ receipts: [['rejected', { state: 'pending' }]] });
    store.append({ receipts: [['rejected', null]] });
    const saved = new ClaudeStateStore(path).load();
    assert.deepEqual(saved.turns.map(t => t.id), ['old', 'new']);
    assert.equal(saved.turns[1].agent_text, 'answer');
    assert.deepEqual(saved.receipts.map(([id]) => id), ['request']);
});
test('Claude journal repairs an incomplete tail before appending and rejects corrupt complete records', t => {
    const path = storeFor(t);
    const store = new ClaudeStateStore(path);
    store.load(() => []);
    store.append({ turn: { id: 'one', status: 'completed' } });
    appendFileSync(path + 'l', '{"turn":');
    const restored = new ClaudeStateStore(path);
    assert.equal(restored.load().turns.length, 1);
    restored.append({ turn: { id: 'two', status: 'completed' } });
    assert.equal(new ClaudeStateStore(path).load().turns.length, 2);
    appendFileSync(path + 'l', 'invalid\n');
    assert.throws(() => new ClaudeStateStore(path).load(), /JSON/);
});
test('Claude incremental persistence grows linearly and preserves delivery deduplication after restart', async t => {
    const path = storeFor(t), store = new ClaudeStateStore(path);
    let bytes = 0;
    const session = new ClaudeSession({
        saved: store.load(() => []), notify: async () => {},
        append: record => { bytes += Buffer.byteLength(JSON.stringify(record)); store.append(record); },
    });
    session.hook(identity);
    let half;
    for (let i = 0; i < 100; i++) {
        const id = 'request-' + i;
        const pending = session.request('prompt', { request_id: id, text: 'question' });
        session.acknowledge(id); await pending;
        session.hook({ ...identity, hook_event_name: 'Stop', last_assistant_message: 'answer'.repeat(100) });
        if (i === 49) half = bytes;
    }
    assert.ok(bytes > 0);
    assert.ok(bytes < half * 2.1, 'later turns must not rewrite earlier history and receipts');
    const saved = new ClaudeStateStore(path).load();
    assert.equal(saved.turns.length, 100);
    assert.equal(saved.receipts.length, 100);
    const resumed = new ClaudeSession({ saved, notify: () => { throw new Error('Duplicate sent'); } });
    resumed.hook(identity);
    assert.deepEqual(await resumed.request('prompt', { request_id: 'request-99', text: 'question' }), { turn_id: saved.turns[99].id });
    assert.ok(readFileSync(path + 'l', 'utf8').includes('answer'));
});
