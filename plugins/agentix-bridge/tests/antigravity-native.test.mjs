import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
const run = promisify(execFile);
const enabled = process.env.AGENTIX_TEST_ANTIGRAVITY_NATIVE === '1';
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";

test('native_agy_hooks_expose_original_conversation_before_model_invocation', { skip: !enabled, timeout: 30000 }, async t => {
    const root = await mkdtemp(join(tmpdir(), 'agentix-agy-native-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const workspace = join(root, 'workspace'), home = join(root, 'home'), events = join(root, 'events.jsonl');
    await mkdir(join(workspace, '.agents'), { recursive: true });
    await mkdir(home);
    const fixture = fileURLToPath(new URL('./fixtures/antigravity-hook.mjs', import.meta.url));
    const hooks = Object.fromEntries(['SessionStart', 'PreInvocation', 'Stop'].map(name => [name,
        [{ type: 'command', command: `${quote(process.execPath)} ${quote(fixture)} ${name}` }]]));
    await writeFile(join(workspace, '.agents/hooks.json'), JSON.stringify({ characterization: hooks }));
    const env = { PATH: process.env.PATH, HOME: home, TMPDIR: root, GEMINI_API_KEY: 'isolated-test-key', ANTIGRAVITY_TEST_EVENTS: events };
    const result = await run(process.env.AGENTIX_TEST_AGY ?? 'agy', ['--settings', '{"modelProvider":"gemini"}', '-p', 'native hook fixture', '--output-format', 'json'],
        { cwd: workspace, env, timeout: 20000, maxBuffer: 1024 * 1024 }).catch(error => error);
    let recorded;
    try { recorded = (await readFile(events, 'utf8')).trim().split('\n').map(JSON.parse); }
    catch { assert.fail(`No native hooks: ${result.stderr ?? result.message ?? result.stdout}`); }
    const start = recorded.find(row => row.name === 'SessionStart');
    const pre = recorded.find(row => row.name === 'PreInvocation');
    assert.ok(start, 'SessionStart is observed');
    assert.ok(pre, 'PreInvocation is observed');
    assert.equal(pre.event.conversationId, start.event.conversationId);
    assert.equal(pre.event.lastUserInput, 'native hook fixture');
    assert.ok(pre.event.transcriptPath.startsWith(home));
    t.diagnostic(JSON.stringify({ events: recorded.map(row => row.name), fields: Object.keys(pre.event), ppid: pre.ppid,
        parents: pre.ancestors.split('\n').filter(line => line.trim().startsWith(String(pre.ppid) + '')) }));
});

test('installed_native_plugins_load_hooks_and_register_original_mcp_host', { skip: !enabled, timeout: 45000 }, async t => {
    const { buildNativePlugins, pluginNames } = await import('../../../scripts/antigravity-plugin.mjs');
    const { createServer } = await import('node:net');
    const { createInterface } = await import('node:readline');
    const root = await mkdtemp(join(tmpdir(), 'agy-loaded-'));
    const workspace = join(root, 'workspace'), home = join(root, 'home'), packages = join(root, 'packages');
    await mkdir(join(workspace, '.agents'), { recursive: true }); await mkdir(home);
    const events = join(root, 'events.jsonl'), log = join(root, 'taskix.jsonl');
    const bin = join(root, 'bin'); await mkdir(bin);
    await writeFile(join(bin, 'taskix'), await readFile(new URL('../../taskix-manager/tests/fixtures/antigravity-taskix-cli.mjs', import.meta.url)), { mode: 0o700 });
    const fixture = fileURLToPath(new URL('./fixtures/antigravity-hook.mjs', import.meta.url));
    await writeFile(join(workspace, '.agents/hooks.json'), JSON.stringify({ fixture: {
        PreInvocation: [{ type: 'command', command: `${quote(process.execPath)} ${quote(fixture)} PreInvocation` }],
    } }));
    const endpoint = join(root, 'control.sock'), frames = [], peers = new Set();
    const server = createServer(socket => {
        peers.add(socket); socket.on('close', () => peers.delete(socket));
        createInterface({ input: socket }).on('line', line => {
            const frame = JSON.parse(line); frames.push(frame);
            if (frame.method === 'register') socket.write(JSON.stringify({ id: 'register', ok: true, result: {} }) + '\n');
        });
    });
    t.after(async () => { for (const peer of peers) peer.destroy(); server.close(); await rm(root, { recursive: true, force: true }); });
    await new Promise(resolve => server.listen(endpoint, resolve));
    const env = { ...process.env, HOME: home, XDG_CONFIG_HOME: join(root, 'config'), XDG_CACHE_HOME: join(root, 'cache'),
        GEMINI_API_KEY: 'isolated-test-key', ANTIGRAVITY_TEST_EVENTS: events,
        AGENTIX_ANTIGRAVITY_DATA_DIR: join(root, 'bridge'), AGENTIX_CONTROL_ENDPOINT: `unix://${endpoint}`,
        TASKIX_ANTIGRAVITY_DATA_DIR: join(root, 'taskix'), TASKIX_TEST_LOG: log,
        PATH: `${bin}:${process.env.PATH}`,
    };
    await buildNativePlugins(fileURLToPath(new URL('../../../', import.meta.url)), packages);
    const agy = process.env.AGENTIX_TEST_AGY ?? 'agy';
    for (const name of pluginNames) await run(agy, ['plugin', 'install', join(packages, name)], { env, timeout: 10000 });
    const result = await run(agy, ['--settings', '{"modelProvider":"gemini"}', '-p', 'installed native plugin fixture', '--output-format', 'json'],
        { cwd: workspace, env, timeout: 25000, maxBuffer: 1024 * 1024 }).catch(error => error);
    const registration = frames.find(frame => frame.method === 'register');
    assert.ok(registration, `Native plugin did not register: ${result.stderr ?? result.message ?? result.stdout}`);
    assert.equal(registration.params.agent, 'antigravity');
    assert.equal(await realpath(registration.params.cwd), await realpath(workspace));
    const recorded = (await readFile(events, 'utf8')).trim().split('\n').map(JSON.parse);
    assert.equal(registration.params.session_id, recorded[0].event.conversationId);
    const calls = (await readFile(log, 'utf8')).trim().split('\n').map(JSON.parse);
    assert.ok(calls.some(args => args.includes('session-start')), 'Installed Taskix SessionStart hook ran');
    assert.ok(calls.some(args => args.includes('session-end')), 'Native MCP EOF released Taskix session');
    t.diagnostic('Both installed native plugins loaded; bridge registered original conversation and Taskix observed startup/shutdown. Inference intentionally terminated.');
});
