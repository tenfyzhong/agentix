import test from 'node:test';
const unixTest = process.platform === 'win32' ? test.skip : test;
import assert from 'node:assert/strict';
import net from 'node:net';
import { once } from 'node:events';
import { mkdtemp, mkdir, readdir, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { registerBridge } from '../runtime.mjs';
import { hostFixture } from './support.mjs';

for (const kind of ['pi', 'omp']) {
    unixTest(`${kind}: extension starts offline and reconnects to the Agentix listener`, { timeout: 5000 }, async t => {
        const directory = await mkdtemp(join(tmpdir(), 'ax-out-'));
        const host = hostFixture();
        const bridge = registerBridge(host.api, kind, { endpoint: `unix://${join(directory, 'control.sock')}`, reconnectDelay: 20 });
        const sockets = new Set();
        let server;
        t.after(async () => { await bridge.close(); for (const socket of sockets) socket.destroy(); if (server) await new Promise(resolve => server.close(resolve)); await rm(directory, { recursive: true, force: true }); });
        await host.emit('session_start');
        assert.deepEqual(await readdir(directory), [], 'extensions must not publish listener/registration files');
        await mkdir(directory, { recursive: true });
        server = net.createServer(socket => { sockets.add(socket); socket.on('error', () => {}); });
        server.listen(join(directory, 'control.sock')); await once(server, 'listening');
        for (let attempt = 0; attempt < 2; attempt++) {
            const [socket] = await once(server, 'connection');
            const [bytes] = await once(socket, 'data');
            const frame = JSON.parse(bytes.toString().trim());
            assert.equal(frame.method, 'register');
            assert.equal(frame.params.token, undefined);
            assert.equal(frame.params.agent, kind);
            assert.equal(frame.params.pid, process.pid);
            assert.equal(frame.params.snapshot.session.id, 'native-id');
            socket.write(JSON.stringify({ id: frame.id, ok: true }) + '\n');
            socket.destroy();
        }
        assert.deepEqual(host.calls, [], 'transport reconnect never replays a prompt');
    });
}
