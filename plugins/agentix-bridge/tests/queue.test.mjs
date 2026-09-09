import test from 'node:test';
import assert from 'node:assert/strict';
import { DurableQueue } from '../queue.mjs';

test('durable queue claims FIFO once and persists before delivery', () => {
    const writes = [];
    const queue = new DurableQueue({}, state => writes.push(state));
    assert.deepEqual(queue.enqueue('a', 'first'), { id: 'a', text: 'first' });
    queue.enqueue('b', 'second');
    queue.enqueue('a', 'first');
    assert.equal(queue.state().queue.length, 2);
    assert.deepEqual(queue.claim(), { id: 'a', text: 'first' });
    assert.equal(writes.at(-1).inflight.id, 'a');
    assert.equal(queue.claim(), null);
    queue.finish();
    assert.equal(queue.claim().id, 'b');
});

test('reloading an in-flight delivery pauses instead of resending', () => {
    const first = new DurableQueue();
    first.enqueue('a', 'first'); first.claim();
    const restored = new DurableQueue(first.state());
    assert.equal(restored.state().uncertain.id, 'a');
    assert.equal(restored.state().paused, true);
    assert.equal(restored.claim(), null);
    assert.throws(() => restored.resume(), error => error.bridgeCode === 'delivery_uncertain');
    restored.clear(true);
    restored.enqueue('b', 'next');
    assert.equal(restored.claim().id, 'b');
});

test('clear preserves active delivery and interruption pauses the remaining queue', () => {
    const queue = new DurableQueue();
    queue.enqueue('a', 'active'); queue.claim(); queue.enqueue('b', 'pending');
    queue.clear(false);
    assert.equal(queue.state().inflight.id, 'a');
    assert.equal(queue.state().queue.length, 0);
    queue.enqueue('c', 'later');
    queue.finish(true);
    assert.equal(queue.claim(), null);
    queue.resume();
    assert.equal(queue.claim().id, 'c');
});

test('saved state and returned snapshots cannot mutate the live queue', () => {
    const queue = new DurableQueue(); queue.enqueue('a', 'original');
    const saved = queue.state(); saved.queue[0].text = 'changed';
    assert.equal(queue.claim().text, 'original');
    const restored = new DurableQueue(queue.state());
    assert.deepEqual(restored.receipt('a'), { id: 'a', text: 'original' });
});

test('queue journals grow linearly while retaining all deduplication receipts', () => {
    let bytes = 0;
    const records = [];
    const queue = new DurableQueue({}, record => { records.push(record); bytes += Buffer.byteLength(JSON.stringify(record)); });
    for (let i = 0; i < 100; i++) { queue.enqueue(String(i), 'prompt'); queue.claim(); queue.finish(); }
    assert.ok(bytes < 200000, `unexpected state write amplification: ${bytes}`);
    const restored = new DurableQueue(DurableQueue.restore(records));
    assert.equal(restored.state().receipts.length, 100);
    assert.equal(restored.state().queue.length, 0);
});

test('journal replay preserves legacy checkpoints and uncertain in-flight work', () => {
    const legacy = { queue: [{ id: 'old', text: 'legacy' }], receipts: [['old', { id: 'old', text: 'legacy' }]], paused: false, inflight: null, uncertain: null };
    const records = [legacy];
    const queue = new DurableQueue(legacy, record => records.push(record));
    queue.claim(); queue.finish(); queue.enqueue('new', 'next'); queue.claim();
    const restored = new DurableQueue(DurableQueue.restore(records));
    assert.equal(restored.state().uncertain.id, 'new');
    assert.deepEqual(restored.receipt('old'), { id: 'old', text: 'legacy' });
});

test('a failed journal write does not acknowledge or consume a delivery', () => {
    let reject = false;
    const queue = new DurableQueue({}, () => { if (reject) throw new Error('disk full'); });
    queue.enqueue('a', 'first');
    reject = true;
    assert.throws(() => queue.claim(), /disk full/);
    assert.equal(queue.state().queue.length, 1);
    assert.equal(queue.state().inflight, null);
    assert.throws(() => queue.enqueue('b', 'second'), /disk full/);
    assert.equal(queue.receipt('b'), undefined);
});

test('duplicate committed journal records never consume the next delivery', () => {
    const records = [];
    const queue = new DurableQueue({}, record => records.push(record));
    queue.enqueue('a', 'first'); queue.enqueue('b', 'second'); queue.claim();
    const restored = new DurableQueue(DurableQueue.restore(records.flatMap(record => [record, record])));
    assert.deepEqual(restored.view().items, [{ id: 'b', text: 'second' }]);
    assert.equal(restored.view().uncertain.id, 'a');
    assert.equal(restored.state().receipts.length, 2);
});
