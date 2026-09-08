import test from 'node:test';
import assert from 'node:assert/strict';
import { createHostAdapter } from '../host.mjs';

test('host adapters settle only at their native terminal event', () => {
    const pi = createHostAdapter({}, 'pi');
    const omp = createHostAdapter({}, 'omp');
    assert.equal(pi.isSettled('agent_end', {}), false);
    assert.equal(pi.isSettled('agent_settled', {}), true);
    assert.equal(omp.isSettled('agent_settled', {}), false);
    assert.equal(omp.isSettled('agent_end', { willContinue: true }), false);
    assert.equal(omp.isSettled('agent_end', { isTerminal: false }), false);
    assert.equal(omp.isSettled('agent_end', { isTerminal: true }), true);
});

test('capabilities are recomputed when the host changes sessions', () => {
    const host = createHostAdapter({ setModel() {} }, 'pi');
    assert.ok(host.capabilities({ compact() {} }).includes('compact'));
    assert.ok(!host.capabilities({}).includes('compact'));
    assert.ok(host.capabilities({}).includes('model'));
    assert.ok(!host.capabilities({}).includes('reasoning'));
});

test('host model enumeration accepts both native registry APIs', async () => {
    const host = createHostAdapter({}, 'omp');
    assert.deepEqual(await host.models({ models: { list: async () => ['omp'] } }), ['omp']);
    assert.deepEqual(await host.models({ modelRegistry: { getAvailable: () => ['pi'] } }), ['pi']);
});

test('IM detachment is not advertised as a native process command', () => {
    for (const kind of ['pi', 'omp']) assert.ok(!createHostAdapter({}, kind).capabilities({}).includes('exit'));
});
