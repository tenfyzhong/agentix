import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import net from 'node:net';
import { once } from 'node:events';
export function hostFixture() {
    const handlers = new Map();
    const calls = [];
    const entries = [];
    const ctx = {
        cwd: '/tmp', hasUI: true, mode: 'tui',
        isIdle: () => !ctx.busy,
        abort: () => { calls.push(['abort']); ctx.busy = false; },
        compact: () => calls.push(['compact']),
        getContextUsage: () => ({ tokens: 12, contextWindow: 100 }),
        sessionManager: { getSessionId: () => 'native-id', getSessionFile: () => '/tmp/session.jsonl', getEntries: () => entries, getBranch: () => entries },
        modelRegistry: { getAvailable: () => [{ provider: 'openai', id: 'test', reasoning: true }] },
        model: { provider: 'openai', id: 'test' },
    };
    const api = {
        on: (name, handler) => handlers.set(name, handler),
        sendUserMessage: (text, options) => { calls.push(['prompt', text, options]); ctx.busy = true; },
        appendEntry: (customType, data) => entries.push({ type: 'custom', customType, data: structuredClone(data) }),
        getSessionName: () => 'Live session', setSessionName: name => calls.push(['rename', name]),
        getThinkingLevel: () => 'high', setThinkingLevel: level => calls.push(['reasoning', level]),
        setModel: model => { calls.push(['model', model]); return true; },
        getCommands: () => [],
    };
    return { api, ctx, calls, entries, async emit(name, event = {}) { await handlers.get(name)?.({ type: name, ...event }, ctx); } };
}
export function peer(socket) {
    const frames = [];
    let buffer = '';
    socket.setEncoding('utf8');
    socket.on('error', () => {});
    socket.on('data', chunk => { buffer += chunk; let at; while ((at = buffer.indexOf('\n')) >= 0) { frames.push(JSON.parse(buffer.slice(0, at))); buffer = buffer.slice(at + 1); } });
    async function request(method, params = {}) {
        const id = String(Math.random());
        socket.write(JSON.stringify({ id, method, params }) + '\n');
        return waitFor(() => frames.find(f => f.id === id));
    }
    return { socket, frames, request };
}
export async function waitFor(find, timeout = 3000) {
    const until = Date.now() + timeout;
    while (Date.now() < until) {
        const value = find(); if (value) return value;
        await new Promise(resolve => setTimeout(resolve, 5));
    }
    throw new Error('Timed out waiting for bridge frame');
}
export async function listen(directory, t) {
    await mkdir(directory, { recursive: true });
    const peers = [], sockets = new Set();
    const server = net.createServer(socket => { sockets.add(socket); peers.push(peer(socket)); });
    server.listen(join(directory, 'control.sock')); await once(server, 'listening');
    t.after(async () => { for (const socket of sockets) socket.destroy(); await new Promise(resolve => server.close(resolve)); });
    return { async next() {
        const client = await waitFor(() => peers.shift(), 20000);
        const frame = await waitFor(() => client.frames.find(f => f.method === 'register'));
        assert.equal(frame.params.token, undefined);
        assert.equal(frame.params.version, 2);
        client.record = frame.params;
        client.socket.write(JSON.stringify({ id: frame.id, ok: true }) + '\n');
        return client;
    } };
}
