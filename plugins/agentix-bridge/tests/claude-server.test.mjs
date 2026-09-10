import test from 'node:test';
import assert from 'node:assert/strict';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, cpSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { Mailbox, hostKey } from '../claude/mailbox.mjs';

for (const [mode, notifications] of [['channel', true], ['rmux', true], ['rmux', false]]) test(`Claude MCP child bridges original session through Agentix socket in ${mode} mode (notifications=${notifications})`, { timeout: 15000, skip: process.platform === 'win32' }, async t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-cc-'));
    cpSync(new URL('../', import.meta.url), join(root, 'plugin'), { recursive: true, filter: path => !path.includes('node_modules') });
    const mailbox = new Mailbox(root, hostKey(process.pid));
    mailbox.publish({ hook_event_name: 'SessionStart', session_id: 'native-claude', cwd: root, transcript_path: join(root, 'native-claude.jsonl') });
    const frames = [], mcp = [];
    let peer;
    const server = net.createServer(socket => {
        peer = socket;
        createInterface({ input: socket }).on('line', line => {
            const frame = JSON.parse(line); frames.push(frame);
            if (frame.method === 'register') socket.write(JSON.stringify({ id: 'register', ok: true, result: {} }) + '\n');
        });
    });
    await new Promise(resolve => server.listen(join(root, 'control.sock'), resolve));
    const preload = notifications ? [] : ['--import', new URL('./silent-watch.mjs', import.meta.url).href];
    const child = spawn(process.execPath, [...preload, join(root, 'plugin/claude/server.mjs')], {
        env: { ...process.env, AGENTIX_CLAUDE_DELIVERY: mode, AGENTIX_CLAUDE_DATA_DIR: root, AGENTIX_CONTROL_ENDPOINT: `unix://${join(root, 'control.sock')}` }, stdio: ['pipe', 'pipe', 'pipe'],
    });
    let errors = ''; child.stderr.on('data', data => errors += data);
    t.after(async () => { child.kill(); peer?.destroy(); server.close(); await new Promise(resolve => child.exitCode !== null ? resolve() : child.once('exit', resolve)); rmSync(root, { recursive: true, force: true }); });
    createInterface({ input: child.stdout }).on('line', line => mcp.push(JSON.parse(line)));
    const send = frame => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...frame }) + '\n');
    async function waitFor(fn) { for (let i = 0; i < 300; i++) { const value = fn(); if (value) return value; if (child.exitCode != null) throw new Error(errors); await new Promise(r => setTimeout(r, 20)); } throw new Error(`timeout: ${errors}`); }
    send({ id: 1, method: 'initialize', params: { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'test', version: '1' } } });
    const initialized = (await waitFor(() => mcp.find(f => f.id === 1))).result;
    if (mode === 'channel') assert.deepEqual(initialized.capabilities.experimental['claude/channel'], {});
    else {
        assert.equal(initialized.capabilities.experimental?.['claude/channel'], undefined);
        assert.equal(initialized.instructions, undefined);
    }
    send({ method: 'notifications/initialized' });
    const registration = await waitFor(() => frames.find(f => f.method === 'register'));
    assert.equal(registration.params.agent, 'claude');
    assert.equal(registration.params.session_id, 'native-claude');
    send({ id: 10, method: 'tools/list', params: {} });
    const tools = (await waitFor(() => mcp.find(f => f.id === 10))).result.tools;
    if (mode === 'rmux') {
        assert.deepEqual(tools, []);
        send({ id: 11, method: 'tools/call', params: { name: 'agentix_acknowledge', arguments: { request_id: 'invented' } } });
        assert.equal((await waitFor(() => mcp.find(f => f.id === 11))).result.isError, true);
        mailbox.publish({ ...mailbox.identity(), hook_event_name: 'UserPromptSubmit', prompt: 'terminal message' });
        mailbox.publish({ ...mailbox.identity(), hook_event_name: 'Stop', last_assistant_message: 'terminal reply' });
        await waitFor(() => frames.find(f => f.event?.TurnCompleted));
        assert.equal(mcp.some(f => f.method === 'notifications/claude/channel'), false);
        return;
    }
    assert.deepEqual(tools.map(tool => tool.name), ['agentix_acknowledge', 'agentix_reply']);
    for (const [id, name, args] of [
        [20, 'unknown_tool', {}],
        [21, 'agentix_acknowledge', { request_id: 'invented' }],
        [22, 'agentix_reply', { request_id: 'invented', text: 'unsolicited' }],
    ]) {
        send({ id, method: 'tools/call', params: { name, arguments: args } });
        assert.equal((await waitFor(() => mcp.find(f => f.id === id))).result.isError, true);
    }
    assert.equal(frames.some(f => f.event?.TurnStarted || f.event?.AgentMessageDelta), false);
    peer.write(JSON.stringify({ id: 'prompt', method: 'prompt', params: { request_id: 'delivery', text: 'hello' } }) + '\n');
    assert.equal((await waitFor(() => mcp.find(f => f.method === 'notifications/claude/channel'))).params.content, 'hello');
    send({ id: 2, method: 'tools/call', params: { name: 'agentix_acknowledge', arguments: { request_id: 'delivery' } } });
    assert.equal((await waitFor(() => frames.find(f => f.id === 'prompt'))).ok, true);
    send({ id: 3, method: 'tools/call', params: { name: 'agentix_reply', arguments: { request_id: 'delivery', text: 'world' } } });
    await waitFor(() => mcp.find(f => f.id === 3));
    mailbox.publish({ ...mailbox.identity(), hook_event_name: 'Stop' });
    assert.equal((await waitFor(() => frames.find(f => f.event?.TurnCompleted))).event.TurnCompleted.status, 'completed');
    peer.destroy();
    await waitFor(() => frames.filter(f => f.method === 'register').length === 2);
    assert.equal(frames.filter(f => f.method === 'register')[1].params.instance, registration.params.instance);
    peer.write(JSON.stringify({ id: 'history', method: 'history', params: { cursor: null, limit: 20 } }) + '\n');
    assert.equal((await waitFor(() => frames.find(f => f.id === 'history'))).result.turns[0].agent_text, 'world');
});
