import test from 'node:test';
import assert from 'node:assert/strict';
import { ObservedUsage } from '../usage.mjs';

test('observed usage deduplicates partial updates and rejects invalid or missing counts', () => {
    const usage = new ObservedUsage();
    assert.deepEqual(usage.total, {});
    usage.record('a', { input: 10 });
    usage.record('a', { input: 10, output: 2 });
    usage.record('a', { input: 9, output: 1 });
    usage.record('b', { input: -1, output: NaN });
    usage.record('c', { input: 1.5, output: '3' });
    assert.deepEqual(usage.total, { input: 10, output: 2 });
    usage.record('b', { input: 5, output: 0 });
    assert.deepEqual(usage.total, { input: 15, output: 2 });
});

test('cached usage remains optional and is deduplicated independently', () => {
    const usage = new ObservedUsage();
    usage.record('a', { input: 10, cached: 7 });
    usage.record('a', { input: 10, cached: 7 });
    usage.record('b', { output: 3 });
    assert.deepEqual(usage.total, { input: 10, output: 3, cached: 7 });
});
