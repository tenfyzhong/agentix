import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, rm, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createServer } from 'node:net';
import { createInterface } from 'node:readline';

const run = promisify(execFile);
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
const enabled = process.env.AGENTIX_TEST_ANTIGRAVITY_MULTIPANE === '1';

test('native_same_directory_panes_register_and_deliver_independently', { skip: !enabled, timeout: 90000 }, async t => {
    const root = await mkdtemp(join(tmpdir(), 'agy-panes-'));
    const workspace = join(root, 'workspace'), bin = join(root, 'bin');
    const endpoint = join(root, 'control.sock'), muxSocket = join(root, 'mux.sock');
    const frames = [], peers = new Set();
    const server = createServer(socket => {
        peers.add(socket); socket.on('close', () => peers.delete(socket));
        createInterface({ input: socket }).on('line', line => {
            const frame = JSON.parse(line); frames.push({ socket, frame });
            if (frame.method === 'register') socket.write(JSON.stringify({ id: frame.id, ok: true, result: { multiplexer: { kind: 'tmux' } } }) + '\n');
        });
    });
    const mux = args => run('tmux', ['-S', muxSocket, ...args], { timeout: 4000 });
    t.after(async () => {
        await mux(['kill-server']).catch(() => {});
        for (const peer of peers) peer.destroy();
        await new Promise(resolve => server.close(resolve));
        await rm(root, { recursive: true, force: true });
    });
    await mkdir(join(workspace, '.agents'), { recursive: true }); await mkdir(bin);
    // Preserve native login, but isolate Taskix calls and all bridge traffic/state.
    await writeFile(join(bin, 'taskix'), await readFile(new URL('../../taskix-manager/tests/fixtures/antigravity-taskix-cli.mjs', import.meta.url)), { mode: 0o700 });
    const hook = fileURLToPath(new URL('./fixtures/antigravity-hook.mjs', import.meta.url));
    const bridgeHook = fileURLToPath(new URL('../antigravity/hook.mjs', import.meta.url));
    await writeFile(join(workspace, '.agents/hooks.json'), JSON.stringify({ fixture: {
        // Publish before terminating inference: termination can skip later plugin hooks.
        PreInvocation: [bridgeHook, hook].map((path, index) => ({ type: 'command',
            command: `${quote(process.execPath)} ${quote(path)}${index ? ' PreInvocation' : ''}` })),
    } }));
    await new Promise(resolve => server.listen(endpoint, resolve));
    const vars = {
        PATH: `${bin}:${process.env.PATH}`, AGENTIX_CONTROL_ENDPOINT: `unix://${endpoint}`,
        AGENTIX_ANTIGRAVITY_DATA_DIR: join(root, 'bridge'), TASKIX_ANTIGRAVITY_DATA_DIR: join(root, 'taskix'),
        TASKIX_TEST_LOG: join(root, 'taskix.jsonl'), ANTIGRAVITY_TEST_EVENTS: join(root, 'events.jsonl'),
    };
    const launch = `exec env ${Object.entries(vars).map(([key, value]) => `${key}=${quote(value)}`).join(' ')} ${quote(process.env.AGENTIX_TEST_AGY ?? 'agy')}`;
    await mux(['-f', '/dev/null', 'new-session', '-d', '-x', '120', '-y', '80', '-c', workspace, launch]);
    await mux(['split-window', '-v', '-c', workspace, launch]);
    const panes = (await mux(['list-panes', '-F', '#{pane_id}|#{pane_pid}'])).stdout.trim().split('\n').map(line => {
        const [pane, pid] = line.split('|'); return { pane, pid: Number(pid) };
    });
    assert.equal(panes.length, 2);
    async function waitFor(action, label) {
        for (let attempt = 0; attempt < 600; attempt++) {
            const value = await action(); if (value) return value;
            await new Promise(resolve => setTimeout(resolve, 50));
        }
        throw new Error(`Timed out: ${label}`);
    }
    for (const pane of panes) await waitFor(async () => {
        const screen = (await mux(['capture-pane', '-p', '-t', pane.pane])).stdout;
        if (screen.includes('Do you trust the contents of this project?')) { await mux(['send-keys', '-t', pane.pane, 'Enter']); return false; }
        assert.ok(!screen.includes('Terms of Service & Data Use'), 'Complete native agy onboarding first');
        return /^(?:-- INSERT -- )?\? for shortcuts[^\S\n]{2,}\S[^\n]*$/m.test(screen);
    }, `pane ${pane.pane} readiness`);
    const initial = [];
    for (const pane of panes) {
        const entry = await waitFor(() => frames.find(({ frame }) => frame.method === 'register' && frame.params.pid === pane.pid), `registration ${pane.pid}`);
        assert.match(entry.frame.params.session_id, /-pending$/);
        assert.equal(await realpath(entry.frame.params.cwd), await realpath(workspace));
        initial.push(entry);
    }
    assert.notEqual(initial[0].frame.params.client_id, initial[1].frame.params.client_id);
    assert.notEqual(initial[0].frame.params.session_id, initial[1].frame.params.session_id);
    for (let index = 0; index < panes.length; index++) {
        const original = initial[index], text = `native pane ${index} fixture`;
        original.socket.write(JSON.stringify({ id: `prompt-${index}`, method: 'prompt', params: { request_id: `request-${index}`, text } }) + '\n');
        const reply = await waitFor(() => frames.find(({ frame }) => frame.id === `prompt-${index}`), `prompt acknowledgement ${index}`);
        if (!reply.frame.ok) {
            t.diagnostic(JSON.stringify(frames.map(({ frame }) => frame)));
            t.diagnostic((await readFile(vars.ANTIGRAVITY_TEST_EVENTS, 'utf8').catch(() => '')).split('\n').filter(Boolean).map(line => JSON.stringify(JSON.parse(line).event)).join('\n'));
            t.diagnostic((await mux(['capture-pane', '-p', '-t', panes[index].pane])).stdout.split('\n').filter(line => !line.includes('@')).join('\n'));
        }
        assert.equal(reply.frame.ok, true, JSON.stringify(reply.frame));
        assert.equal(reply.socket, original.socket);
        const native = await waitFor(() => frames.find(({ frame }) => frame.method === 'register' && frame.params.previous_session_id === original.frame.params.session_id), `native handoff ${index}`);
        assert.equal(native.frame.params.pid, panes[index].pid);
        assert.equal(native.frame.params.client_id, original.frame.params.client_id);
        assert.ok(reply.frame.result.turn_id.startsWith(native.frame.params.session_id + ':'));
        const witness = await waitFor(async () => {
            const contents = await readFile(vars.ANTIGRAVITY_TEST_EVENTS, 'utf8').catch(() => '');
            return contents.trim().split('\n').filter(Boolean).map(JSON.parse)
                .find(row => row.event.conversationId === native.frame.params.session_id);
        }, `native input witness ${index}`);
        assert.equal(witness.event.lastUserInput, text);
        native.socket.destroy();
        const reconnected = await waitFor(() => frames.find(({ socket, frame }) => socket !== native.socket
            && frame.method === 'register' && frame.params.session_id === native.frame.params.session_id), `reconnect ${index}`);
        assert.equal(reconnected.frame.params.client_id, original.frame.params.client_id);
        assert.equal(reconnected.frame.params.pid, panes[index].pid);
        reconnected.socket.write(JSON.stringify({ id: `retry-${index}`, method: 'prompt',
            params: { request_id: `request-${index}`, text } }) + '\n');
        const retry = await waitFor(() => frames.find(({ frame }) => frame.id === `retry-${index}`), `replayed receipt ${index}`);
        assert.equal(retry.frame.ok, true);
        assert.deepEqual(retry.frame.result, reply.frame.result);
        reconnected.socket.write(JSON.stringify({ id: `history-${index}`, method: 'history', params: {} }) + '\n');
        const history = await waitFor(() => frames.find(({ frame }) => frame.id === `history-${index}`), `history ${index}`);
        assert.equal(history.frame.ok, true);
        assert.deepEqual(history.frame.result.turns.map(turn => turn.user_text), [text]);
        if (index === 0) assert.ok(!frames.some(({ frame }) => frame.method === 'register' && frame.params.previous_session_id === initial[1].frame.params.session_id));
    }
    t.diagnostic('Two native panes in one directory remained distinct before input, acknowledged their own first prompt through original sockets, and replayed receipts after reconnect without adding a turn. Inference intentionally terminated.');
});
