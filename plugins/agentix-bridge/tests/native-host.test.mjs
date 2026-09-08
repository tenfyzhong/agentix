import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { listen } from './support.mjs';

for (const kind of ['pi', 'omp']) {
    test(`${kind}: installed host loads the bridge in its original process`, { skip: process.platform === 'win32' || !process.env.AGENTIX_TEST_NATIVE_HOSTS, timeout: 25000 }, async t => {
        const directory = await mkdtemp(join(tmpdir(), 'ax-native-'));
        const server = await listen(directory, t);
        const args = ['--mode', 'rpc', '--no-extensions', '--no-skills', '--session-dir', join(directory, 'sessions'), '-e', fileURLToPath(new URL(`../extensions/${kind}.ts`, import.meta.url))];
        if (kind === 'pi') args.push('--offline', '--no-context-files');
        else args.push('--no-lsp', '--no-title', '--no-rules');
        const host = spawn(kind, args, { cwd: directory, env: { ...process.env, PI_CODING_AGENT_DIR: join(directory, 'config'), AGENTIX_CONTROL_ENDPOINT: `unix://${join(directory, 'control.sock')}`, PI_OFFLINE: '1', OPENAI_API_KEY: 'test-only-no-model-requests' }, stdio: ['pipe', 'pipe', 'pipe'] });
        let output = ''; host.stdout.on('data', data => { output += data; }); host.stderr.on('data', data => { output += data; });
        t.after(async () => { if (host.exitCode === null) { host.kill('SIGTERM'); await once(host, 'exit'); } await rm(directory, { recursive: true, force: true }); });
        const client = await server.next();
        const record = client.record;
        assert.equal(record.pid, host.pid);
        assert.equal(typeof record.session_file, "string");
        assert.ok(record.session_file.startsWith(join(directory, "sessions")));
        t.after(() => client.socket.destroy());
        const hello = await client.request('snapshot');
        assert.equal(hello.ok, true);
        assert.equal(typeof hello.result.session.id, 'string');
        assert.equal((await client.request('command', { name: 'rename', value: 'Native bridge smoke' })).ok, true);
        assert.equal((await client.request('command', { name: 'skills' })).ok, true);
        assert.equal((await client.request('command', { name: 'model' })).ok, true);
        assert.equal((await client.request('queue_state')).result.items.length, 0);
        client.socket.destroy();
        assert.equal(host.exitCode, null);
    });
}
