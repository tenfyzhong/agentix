import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { listen } from './support.mjs';
import { once } from 'node:events';

test('installed Claude marketplace plugin registers its original host session', {
    skip: !process.env.AGENTIX_TEST_NATIVE_HOSTS || process.platform === 'win32', timeout: 120000,
}, async t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-cn-'));
    let child;
    let client;
    t.after(async () => {
        client?.socket.destroy();
        if (child?.pid && child.exitCode === null && child.signalCode === null) {
            const exited = once(child, 'exit');
            const kill = signal => {
                try { process.kill(-child.pid, signal); }
                catch (error) { if (error.code !== 'ESRCH') throw error; }
            };
            kill('SIGTERM');
            const deadline = setTimeout(() => kill('SIGKILL'), 1000);
            try { await exited; } finally { clearTimeout(deadline); }
        }
        // Stop the isolated host and its MCP children before deleting files
        // they can still write during shutdown.
        rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    });
    const env = { ...process.env, CLAUDE_CONFIG_DIR: join(root, 'config'), AGENTIX_CLAUDE_DATA_DIR: join(root, 'data'),
        AGENTIX_CONTROL_ENDPOINT: `unix://${join(root, 'control.sock')}`, ANTHROPIC_API_KEY: 'test-only',
        ANTHROPIC_BASE_URL: 'http://127.0.0.1:1', CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1' };
    const repo = new URL('../../../', import.meta.url).pathname;
    execFileSync('claude', ['plugin', 'marketplace', 'add', repo], { env, cwd: root, timeout: 60000 });
    execFileSync('claude', ['plugin', 'install', 'agentix-bridge@agentix'], { env, cwd: root, timeout: 60000 });
    const server = await listen(root, t);
    child = spawn('claude', ['-p', '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose'], { env, cwd: root, detached: true, stdio: ['pipe', 'pipe', 'pipe'] });
    let output = ''; child.stdout.on('data', value => output += value); child.stderr.on('data', value => output += value);
    child.stdin.write(JSON.stringify({ type: 'user', message: { role: 'user', content: 'Connection test only.' } }) + '\n');
    let deadline;
    client = await Promise.race([server.next(), new Promise((_, reject) => { deadline = setTimeout(() => reject(new Error(output)), 20000); })]).finally(() => clearTimeout(deadline));
    assert.equal(client.record.agent, 'claude');
    assert.equal(client.record.pid, child.pid);
    assert.equal((await client.request('snapshot')).ok, true);
});
