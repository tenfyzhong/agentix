import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { BridgeTransport } from '../transport.mjs';
import { listen, waitFor } from './support.mjs';
const unixTest = process.platform === 'win32' ? test.skip : test;

unixTest('transport frames fragmented requests and scopes events to a connection', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-transport-'));
    const server = await listen(directory, t);
    const calls = [];
    const transport = new BridgeTransport({
        endpoint: `unix://${join(directory, 'control.sock')}`,
        snapshot: () => ({}),
        dispatch: work => work(),
        handle: async (method, params) => { calls.push([method, params]); return { body: 'ok' }; },
    });
    t.after(async () => { await transport.close(); await rm(directory, { recursive: true, force: true }); });
    transport.open({ instance: 'one', agent: 'pi', session_id: 'native' });
    const client = await server.next();
    client.socket.write('{"id":"request","method":"status",');
    client.socket.write('"params":{}}\n');
    const response = await waitFor(() => client.frames.find(frame => frame.id === 'request'));
    assert.deepEqual(response.result, { body: 'ok' });
    assert.deepEqual(calls, [['status', {}]]);
    transport.event({ QueueChanged: { session_id: 'native' } });
    const event = await waitFor(() => client.frames.find(frame => frame.event));
    assert.equal(event.instance, 'one'); assert.equal(event.seq, 1);
    await transport.close();
    transport.event({ QueueChanged: { session_id: 'native' } });
    assert.equal(transport.sequence, 1);
});

unixTest('oversized command responses return a bounded protocol error', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-frame-'));
    const server = await listen(directory, t);
    const { MAX_FRAME } = await import('../transport.mjs');
    const transport = new BridgeTransport({ endpoint: `unix://${join(directory, 'control.sock')}`,
        snapshot: () => ({}), dispatch: work => work(), handle: async () => ({ text: 'x'.repeat(MAX_FRAME + 1) }) });
    t.after(async () => { await transport.close(); await rm(directory, { recursive: true, force: true }); });
    transport.open({ instance: 'large', agent: 'pi', session_id: 'native' });
    const client = await server.next();
    const response = await client.request('large');
    assert.equal(response.ok, false);
    assert.equal(response.code, 'frame_too_large');
});
