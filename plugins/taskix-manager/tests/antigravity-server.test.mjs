import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { createAntigravityHooks, hostKey } from '../antigravity/runtime.mjs';

test('native_hook_entrypoint_returns_native_context_json', { timeout: 10000, skip: process.platform === 'win32' }, async t => {
    const root = await mkdtemp(join(tmpdir(), 'taskix-agy-hook-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const bin = join(root, 'bin'); await mkdir(bin);
    await writeFile(join(bin, 'taskix'), await readFile(new URL('./fixtures/antigravity-taskix-cli.mjs', import.meta.url)), { mode: 0o700 });
    const child = spawn(process.execPath, [new URL('../antigravity/hook.mjs', import.meta.url).pathname], {
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TASKIX_TEST_LOG: join(root, 'calls.jsonl'), TASKIX_ANTIGRAVITY_DATA_DIR: root },
        stdio: ['pipe', 'pipe', 'pipe'],
    });
    let output = '', errors = '';
    child.stdout.on('data', chunk => output += chunk); child.stderr.on('data', chunk => errors += chunk);
    const done = new Promise(resolve => child.once('exit', resolve));
    child.stdin.end(JSON.stringify({ hookEventName: 'SessionStart', conversationId: 'native', workspacePaths: [root], transcriptPath: join(root, 'transcript.jsonl') }));
    assert.equal(await done, 0, errors);
    const result = JSON.parse(output);
    assert.match(result.injectSteps[0].ephemeralMessage, /Task session: native/);
    assert.equal(result.hookSpecificOutput, undefined);
});

test('native_mcp_tool_uses_original_session_and_releases_on_stdio_close', { timeout: 10000, skip: process.platform === 'win32' }, async t => {
    const root = await mkdtemp(join(tmpdir(), 'taskix-agy-mcp-'));
    const bin = join(root, 'bin'), log = join(root, 'calls.jsonl');
    await mkdir(bin);
    await writeFile(join(bin, 'taskix'), await readFile(new URL('./fixtures/antigravity-taskix-cli.mjs', import.meta.url)), { mode: 0o700 });
    const hooks = createAntigravityHooks({ root, key: hostKey(process.pid), runner: async () => ({ result: {} }), routing: { env: {} } });
    await writeFile(join(root, 'transcript.jsonl'), '');
    await hooks.handle({ hookEventName: 'SessionStart', conversationId: 'native', workspacePaths: [root], transcriptPath: join(root, 'transcript.jsonl') });
    const child = spawn(process.execPath, [new URL('../antigravity/server.mjs', import.meta.url).pathname], {
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TASKIX_TEST_LOG: log, TASKIX_ANTIGRAVITY_DATA_DIR: root },
        stdio: ['pipe', 'pipe', 'pipe'],
    });
    const frames = []; let errors = '';
    child.stderr.on('data', chunk => errors += chunk);
    createInterface({ input: child.stdout }).on('line', line => frames.push(JSON.parse(line)));
    t.after(async () => { child.kill(); if (child.exitCode === null && child.signalCode === null) await new Promise(resolve => child.once('exit', resolve)); await rm(root, { recursive: true, force: true }); });
    const send = frame => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...frame }) + '\n');
    async function waitFor(predicate) {
        for (let i = 0; i < 300; i++) {
            const result = predicate(); if (result) return result;
            if (child.exitCode !== null) throw new Error(errors);
            await new Promise(resolve => setTimeout(resolve, 10));
        }
        throw new Error(`MCP response missing: ${errors}`);
    }
    send({ id: 1, method: 'initialize', params: { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'test', version: '1' } } });
    await waitFor(() => frames.find(frame => frame.id === 1));
    send({ method: 'notifications/initialized' });
    send({ id: 2, method: 'tools/list', params: {} });
    assert.equal((await waitFor(() => frames.find(frame => frame.id === 2))).result.tools[0].name, 'taskix');
    send({ id: 3, method: 'tools/call', params: { name: 'taskix', arguments: { args: ['task', 'done', 'task_fixture'] } } });
    const response = (await waitFor(() => frames.find(frame => frame.id === 3))).result;
    const args = JSON.parse(response.content[0].text).result.args;
    assert.equal(args[args.indexOf('--session') + 1], 'native');
    assert.equal(args[args.indexOf('--executor') + 1], 'agent:antigravity:native');
    assert.equal(args[args.indexOf('--lease-token') + 1], 'lease_fixture');
    const exited = new Promise(resolve => child.once('exit', resolve));
    child.stdin.end();
    await exited;
    const calls = (await readFile(log, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
    assert.ok(calls.some(args => args.includes('session-end') && args.includes('native')));
});
