import test from 'node:test';
import assert from 'node:assert/strict';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { AntigravityMailbox } from '../antigravity/mailbox.mjs';
import { hostKey } from '../claude/mailbox.mjs';

for (const empty of [false, true]) test(`antigravity_mcp_registers_original_process_empty_${empty}`, { timeout: 10000, skip: process.platform === 'win32' }, async t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-agy-'));
    const transcript = join(root, 'transcript.jsonl');
    writeFileSync(transcript, JSON.stringify({ step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'hello' }) + '\n');
    const mailbox = new AntigravityMailbox(root, hostKey(process.pid));
    if (!empty) mailbox.publish({ hookEventName: 'SessionStart', conversationId: 'native', workspacePaths: [root], transcriptPath: transcript });
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
    const child = spawn(process.execPath, [new URL('../antigravity/server.mjs', import.meta.url).pathname], {
        cwd: root,
        env: { ...process.env, AGENTIX_ANTIGRAVITY_DATA_DIR: root, AGENTIX_CONTROL_ENDPOINT: `unix://${join(root, 'control.sock')}` },
        stdio: ['pipe', 'pipe', 'pipe'],
    });
    let errors = ''; child.stderr.on('data', chunk => errors += chunk);
    t.after(async () => {
        child.kill(); peer?.destroy(); server.close();
        if (child.exitCode === null && child.signalCode === null) await new Promise(resolve => child.once('exit', resolve));
        rmSync(root, { recursive: true, force: true });
    });
    createInterface({ input: child.stdout }).on('line', line => mcp.push(JSON.parse(line)));
    async function waitFor(fn) {
        for (let i = 0; i < 300; i++) {
            const result = fn(); if (result) return result;
            if (child.exitCode !== null) throw new Error(errors);
            await new Promise(resolve => setTimeout(resolve, 10));
        }
        throw new Error(`Timed out: ${errors}`);
    }
    const send = frame => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...frame }) + '\n');
    send({ id: 1, method: 'initialize', params: { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'test', version: '1' } } });
    await waitFor(() => mcp.find(frame => frame.id === 1));
    send({ method: 'notifications/initialized' });
    const registration = await waitFor(() => frames.find(frame => frame.method === 'register'));
    assert.equal(registration.params.agent, 'antigravity');
    assert.ok(registration.params.snapshot.capabilities.includes('new'));
    assert.equal(registration.params.pid, process.pid);
    if (empty) {
        assert.ok(registration.params.session_id.endsWith('-pending'));
        assert.equal(registration.params.cwd, process.cwd());
        return;
    }
    assert.equal(registration.params.session_id, 'native');
    send({ id: 2, method: 'tools/list', params: {} });
    assert.deepEqual((await waitFor(() => mcp.find(frame => frame.id === 2))).result.tools, []);
    mailbox.publish({ ...mailbox.identity(), hookEventName: 'PreInvocation' });
    mailbox.publish({ ...mailbox.identity(), hookEventName: 'Stop', fullyIdle: true });
    await waitFor(() => frames.find(frame => frame.event?.TurnCompleted));
    peer.write(JSON.stringify({ id: 'history', method: 'history', params: { limit: 20 } }) + '\n');
    const response = await waitFor(() => frames.find(frame => frame.id === 'history'));
    assert.equal(response.result.turns[0].user_text, 'hello');
    assert.equal(response.result.turns[0].status, 'completed');
});
