import test from 'node:test';
const unixTest = process.platform === 'win32' ? test.skip : test;
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { registerBridge } from '../runtime.mjs';

import { hostFixture, listen, waitFor } from './support.mjs';
for (const kind of ['pi', 'omp']) {
    unixTest(`${kind}: original session outbound IPC streams final settle only`, async t => {
        const directory = await mkdtemp(join(tmpdir(), 'ax-'));
        const server = await listen(directory, t);
        const host = hostFixture();
        const bridge = registerBridge(host.api, kind, { endpoint: `unix://${join(directory, 'control.sock')}` });
        t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
        await host.emit('session_start');
        const client = await server.next();
        const record = client.record;
        assert.equal(record.pid, process.pid);
        assert.equal(record.session_id, 'native-id');
        t.after(() => client.socket.destroy());
        assert.equal((await client.request('prompt', { text: 'hello', request_id: 'one' })).ok, true);
        assert.equal(host.calls[0][1], 'hello');
        await host.emit('agent_start');
        await host.emit('message_update', { assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: 'answer' } });
        await host.emit('agent_end', { messages: [], willContinue: true, isTerminal: false });
        await new Promise(resolve => setTimeout(resolve, 20));
        assert.equal(client.frames.filter(f => f.event?.TurnCompleted).length, 0);
        host.ctx.busy = false;
        await host.emit(kind === 'pi' ? 'agent_settled' : 'agent_end', { messages: [], willContinue: false, isTerminal: true });
        await new Promise(resolve => setTimeout(resolve, 20));
        assert.equal(client.frames.filter(f => f.event?.TurnCompleted).length, 1);
        assert.equal(client.frames.filter(f => f.event?.AgentMessageDelta)[0].event.AgentMessageDelta.delta, 'answer');
        client.socket.destroy();
        const again = await server.next(); t.after(() => again.socket.destroy());
        const hello = await again.request('snapshot');
        assert.equal(hello.result.session.id, 'native-id');
        assert.equal(host.calls.filter(c => c[0] === 'prompt').length, 1);
    });
}

test('repository installation loads one task extension and one bridge for each host', async () => {
    const pkg = JSON.parse(await readFile(new URL('../../../package.json', import.meta.url), 'utf8'));
    for (const host of ['pi', 'omp']) {
        assert.ok(pkg[host].extensions.includes(`./plugins/agentix-bridge/extensions/${host}.ts`));
        const entry = await import(`../extensions/${host}.ts`);
        const fixture = hostFixture();
        const bridge = entry.default(fixture.api);
        assert.equal(typeof bridge.close, 'function');
        await bridge.close();
    }
});

for (const kind of ['pi', 'omp']) {
    unixTest(`${kind}: FIFO persists, deduplicates, and pauses on interruption`, async t => {
        const directory = await mkdtemp(join(tmpdir(), 'ax-q-'));
        const server = await listen(directory, t);
        const host = hostFixture();
        let bridge = registerBridge(host.api, kind, { endpoint: `unix://${join(directory, 'control.sock')}` });
        t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
        async function clientForHost() {
            const client = await server.next(); t.after(() => client.socket.destroy());
            assert.equal((await client.request('snapshot')).ok, true);
            return client;
        }
        await host.emit('session_start');
        let client = await clientForHost();
        await client.request('prompt', { text: 'first', request_id: 'first' });
        for (const id of ['second', 'third', 'second']) assert.equal((await client.request('queue', { text: id, request_id: id })).ok, true);
        assert.deepEqual((await client.request('queue_state')).result.items.map(p => p.text), ['second', 'third']);
        assert.equal(host.calls.filter(c => c[0] === 'prompt').length, 1);
        await client.request('stop');
        await host.emit(kind === 'pi' ? 'agent_settled' : 'agent_end');
        assert.equal((await client.request('queue_state')).result.paused, true);
        await bridge.close();
        bridge = registerBridge(host.api, kind, { endpoint: `unix://${join(directory, 'control.sock')}` });
        await host.emit('session_start');
        client = await clientForHost();
        assert.deepEqual((await client.request('queue_state')).result.items.map(p => p.text), ['second', 'third']);
        await client.request('queue_resume');
        assert.equal(host.calls.filter(c => c[0] === 'prompt').at(-1)[1], 'second');
        host.ctx.busy = false;
        await host.emit(kind === 'pi' ? 'agent_settled' : 'agent_end');
        await new Promise(resolve => setTimeout(resolve, 20));
        assert.equal(host.calls.filter(c => c[0] === 'prompt').at(-1)[1], 'third');
    });
}

unixTest('native controls validate models and expose only supported capabilities', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-c-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    const bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next(); t.after(() => client.socket.destroy());
    const hello = await client.request('snapshot');
    assert.ok(hello.result.capabilities.includes('model'));
    assert.ok(!hello.result.capabilities.includes('plan'));
    const models = await client.request('command', { name: 'model' });
    assert.equal(models.result.choices[0].value, 'openai/test');
    assert.equal((await client.request('command', { name: 'model', value: 'made-up' })).ok, false);
    assert.equal((await client.request('command', { name: 'model', value: 'openai/test' })).ok, true);
    assert.equal((await client.request('command', { name: 'rename', value: 'new title' })).ok, true);
    assert.ok(host.calls.some(c => c[0] === 'rename' && c[1] === 'new title'));
});

unixTest('forked sessions do not inherit queued prompts and clear preserves an active delivery', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-f-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    const bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next(); t.after(() => client.socket.destroy());
    await client.request('snapshot');
    await client.request('prompt', { text: 'active', request_id: 'active' });
    await client.request('queue', { text: 'pending', request_id: 'pending' });
    await client.request('queue_clear');
    assert.equal(host.entries.at(-1).data.inflight.id, 'active');
    await client.request('queue', { text: 'parent-only', request_id: 'parent-only' });
    host.ctx.sessionManager.getSessionId = () => 'fork-id';
    await host.emit('session_start');
    assert.deepEqual(bridge.snapshot().queue.items, []);
});

unixTest('skills support asynchronous native command enumeration', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-s-'));
    const server = await listen(directory, t);
    const host = hostFixture(); host.api.getCommands = async () => [{ name: 'skill:review', source: 'skill' }];
    const bridge = registerBridge(host.api, 'omp', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next(); t.after(() => client.socket.destroy());
    await client.request('snapshot');
    const result = await client.request('command', { name: 'skills' });
    assert.equal(result.ok, true);
    assert.match(result.result.body, /skill:review/);
});

unixTest('reload preserves active turn identity and reports uncertain delivery without resending', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-reload-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    let bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next(); t.after(() => client.socket.destroy());
    await client.request('snapshot');
    const result = await client.request('prompt', { text: 'active', request_id: 'active' });
    await bridge.close();
    bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    await host.emit('session_start');
    assert.equal(bridge.snapshot().turns.at(-1)?.id, result.result.turn_id);
    assert.equal(bridge.snapshot().queue.uncertain.id, 'active');
    assert.equal(host.calls.filter(c => c[0] === 'prompt').length, 1);
});

unixTest('tool lifecycle streams to IM and shutdown reports session exit', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-tools-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    const bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next(); t.after(() => client.socket.destroy());
    await client.request('snapshot');
    await client.request('prompt', { text: 'inspect', request_id: 'inspect' });
    await host.emit('tool_execution_start', { toolCallId: 'call-1', toolName: 'read', args: { path: 'README.md' } });
    await host.emit('tool_execution_end', { toolCallId: 'call-1', toolName: 'read', isError: false, result: { content: [{ type: 'text', text: 'contents' }] } });
    await client.request('snapshot');
    assert.ok(client.frames.some(f => f.event?.ItemStarted?.item_id === 'call-1'));
    assert.ok(client.frames.some(f => f.event?.ItemCompleted?.item.id === 'call-1'));
    assert.equal(bridge.snapshot().turns.at(-1).tools[0].status, 'completed');
    await host.emit('session_shutdown');
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.ok(client.frames.some(f => f.event?.SessionExited));
});

unixTest('an identical new prompt never steals the preceding history entry', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-history-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    host.entries.push({ type: 'message', id: 'old-user', message: { role: 'user', content: 'repeat', timestamp: 1 } });
    const bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next(); t.after(() => client.socket.destroy());
    await client.request('snapshot');
    const started = await client.request('prompt', { text: 'repeat', request_id: 'new' });
    await host.emit('message_start', { message: { role: 'user', content: 'repeat', timestamp: 2 } });
    assert.equal(bridge.snapshot().turns[0].id, 'old-user');
    host.entries.push({ type: 'message', id: 'new-user', message: { role: 'user', content: 'repeat', timestamp: 2 } });
    host.ctx.busy = false;
    await host.emit('agent_settled');
    assert.deepEqual(bridge.snapshot().turns.map(turn => turn.id), ['old-user', started.result.turn_id]);
});

unixTest('history requests return bounded pages with stable cursors', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-pages-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    for (let i = 0; i < 45; i++) host.entries.push({ type: 'message', id: `user-${i}`, message: { role: 'user', content: `prompt ${i}`, timestamp: i } });
    const bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next(); t.after(() => client.socket.destroy());
    const hello = await client.request('snapshot');
    assert.ok(hello.result.turns.length <= 20);
    const latest = (await client.request('history', { limit: 2 })).result;
    assert.deepEqual(latest.turns.map(turn => turn.id), ['user-43', 'user-44']);
    const older = (await client.request('history', { limit: 2, cursor: latest.older_cursor })).result;
    assert.deepEqual(older.turns.map(turn => turn.id), ['user-41', 'user-42']);
    assert.equal(older.newer_cursor, '45');
});

unixTest('wire failures expose stable error codes without parsing prose', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-errors-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    const bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next();
    assert.equal((await client.request('unknown')).code, 'unsupported_method');
    assert.equal((await client.request('command', { name: 'plan' })).code, 'unsupported_command');
    assert.equal((await client.request('prompt', { text: '', request_id: 'invalid' })).code, 'invalid_request');
    host.ctx.busy = true;
    assert.equal((await client.request('prompt', { text: 'hello', request_id: 'busy' })).code, 'busy');
});

unixTest('queued delivery persists its turn identity before invoking the host', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-queued-turn-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    let deliveredState;
    host.api.sendUserMessage = () => {
        deliveredState = host.entries.findLast(e => e.customType === 'agentix.bridge')?.data;
        host.ctx.busy = true;
    };
    const bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next();
    await client.request('queue', { text: 'queued', request_id: 'queued-id' });
    const snapshot = await client.request('snapshot');
    assert.equal(deliveredState.inflight.id, 'queued-id');
    assert.equal(deliveredState.turn?.id, snapshot.result.turns.at(-1).id);
});

async function connectedFixture(t) {
    const directory = await mkdtemp(join(tmpdir(), 'ax-review-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    const bridge = registerBridge(host.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    return { host, bridge, server, client: await server.next() };
}

unixTest('idle sessions reject steering without invoking the host', async t => {
    const { host, client } = await connectedFixture(t);
    const response = await client.request('steer', { text: 'hello', request_id: 'idle' });
    assert.equal(response.ok, false);
    assert.equal(response.code, 'invalid_request');
    assert.equal(host.calls.length, 0);
});

unixTest('failed prompt retries return the original failure without another delivery', async t => {
    const { host, client } = await connectedFixture(t);
    let attempts = 0;
    host.api.sendUserMessage = () => { attempts++; throw new Error('native delivery failed'); };
    const first = await client.request('prompt', { text: 'hello', request_id: 'failure' });
    const second = await client.request('prompt', { text: 'hello', request_id: 'failure' });
    assert.equal(first.ok, false); assert.equal(second.ok, false);
    assert.equal(second.error, first.error); assert.equal(attempts, 1);
});

unixTest('request IDs cannot be reused for a different queued prompt', async t => {
    const { host, client } = await connectedFixture(t);
    host.ctx.busy = true;
    await client.request('queue', { text: 'first', request_id: 'same' });
    const changed = await client.request('queue', { text: 'different', request_id: 'same' });
    assert.equal(changed.ok, false);
    assert.equal(changed.code, 'invalid_request');
    assert.equal((await client.request('queue_state')).result.items[0].text, 'first');
});

unixTest('clearing an uncertain delivery allows a fresh turn identity', async t => {
    const { host, client, server } = await connectedFixture(t);
    const first = await client.request('prompt', { text: 'first', request_id: 'first' });
    host.ctx.busy = false;
    await host.emit('session_start');
    const reloaded = await server.next();
    assert.equal((await reloaded.request('queue_state')).result.uncertain.id, 'first');
    await reloaded.request('queue_clear');
    const next = await reloaded.request('prompt', { text: 'next', request_id: 'next' });
    assert.notEqual(next.result.turn_id, first.result.turn_id);
});


unixTest('a model lookup finishing after session switch cannot change the new session', async t => {
    const { host, client, server } = await connectedFixture(t);
    let complete;
    host.ctx.modelRegistry.getAvailable = () => new Promise(resolve => { complete = resolve; });
    client.socket.write(JSON.stringify({ id: 'old-model', method: 'command', params: { name: 'model', value: 'openai/test' } }) + '\n');
    await waitFor(() => complete);
    host.ctx.sessionManager.getSessionId = () => 'second';
    await host.emit('session_start');
    const next = await server.next();
    await next.request('snapshot');
    complete([{ provider: 'openai', id: 'test' }]);
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(host.calls.filter(call => call[0] === 'model').length, 0);
});

unixTest('an old delivery rejection cannot pause or overwrite a new session', async t => {
    const { host, client, server } = await connectedFixture(t);
    let reject;
    host.api.sendUserMessage = () => new Promise((_resolve, fail) => { reject = fail; });
    client.socket.write(JSON.stringify({ id: 'old-prompt', method: 'prompt', params: { text: 'old', request_id: 'old' } }) + '\n');
    await waitFor(() => reject);
    host.ctx.sessionManager.getSessionId = () => 'second';
    await host.emit('session_start');
    const next = await server.next();
    reject(new Error('old delivery failed'));
    const state = (await next.request('queue_state')).result;
    assert.equal(state.paused, false); assert.equal(state.uncertain, null);
    assert.equal(host.entries.filter(e => e.data?.session_id === 'second').length, 0);
});

unixTest('stop and state reads bypass an unrelated pending host command', async t => {
    const { host, client } = await connectedFixture(t);
    let complete;
    host.ctx.modelRegistry.getAvailable = () => new Promise(resolve => { complete = resolve; });
    t.after(() => complete?.([]));
    client.socket.write(JSON.stringify({ id: 'slow-models', method: 'command', params: { name: 'model' } }) + '\n');
    await waitFor(() => complete);
    assert.equal((await client.request('stop')).ok, true);
    assert.equal((await client.request('queue_state')).result.paused, true);
    assert.equal(host.calls.filter(call => call[0] === 'abort').length, 1);
    complete([]);
});

unixTest('failed persistence never announces or submits a remote turn', async t => {
    const { host, client } = await connectedFixture(t);
    host.api.appendEntry = () => { throw new Error('disk full'); };
    const response = await client.request('prompt', { text: 'must not run', request_id: 'no-write' });
    assert.equal(response.ok, false);
    assert.equal(host.calls.filter(call => call[0] === 'prompt').length, 0);
    assert.equal(client.frames.filter(frame => frame.event?.TurnStarted).length, 0);
});

unixTest('session metadata avoids projecting history and serializing the queue', async t => {
    const { host, client } = await connectedFixture(t);
    host.ctx.sessionManager.getBranch = () => { throw new Error('history must not be scanned'); };
    const response = await client.request('info');
    assert.equal(response.ok, true);
    assert.equal(response.result.session.id, host.ctx.sessionManager.getSessionId());
    assert.equal(response.result.turns, undefined);
    assert.equal(response.result.queue, undefined);
});

unixTest('registration remains available when native history cannot be projected', async t => {
    const directory = await mkdtemp(join(tmpdir(), 'ax-info-'));
    const server = await listen(directory, t);
    const host = hostFixture();
    host.ctx.sessionManager.getBranch = () => { throw new Error('history unavailable'); };
    const bridge = registerBridge(host.api, 'omp', { endpoint: `unix://${join(directory, 'control.sock')}` });
    t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
    await host.emit('session_start');
    const client = await server.next();
    assert.equal(client.record.snapshot.turns, undefined);
    assert.equal((await client.request('info')).ok, true);
    assert.equal((await client.request('history')).ok, false);
});

for (const kind of ['pi', 'omp']) {
    unixTest(`${kind}: emits reasoning and tool input for process output`, async t => {
        const directory = await mkdtemp(join(tmpdir(), 'ax-process-'));
        const server = await listen(directory, t);
        const host = hostFixture();
        const bridge = registerBridge(host.api, kind, { endpoint: `unix://${join(directory, 'control.sock')}` });
        t.after(async () => { await bridge.close(); await rm(directory, { recursive: true, force: true }); });
        await host.emit('session_start');
        const client = await server.next();
        await client.request('snapshot');
        await host.emit('message_update', { assistantMessageEvent: { type: 'thinking_end', contentIndex: 0, content: 'Visible reasoning' } });
        await host.emit('tool_execution_start', { toolCallId: 'tool', toolName: 'bash', args: { command: 'pwd' } });
        await host.emit('tool_execution_end', { toolCallId: 'tool', toolName: 'bash', result: { content: [{ type: 'text', text: '/tmp' }] } });
        const reasoning = await waitFor(() => client.frames.find(f => f.event?.ItemCompleted?.item.kind === 'reasoning'));
        assert.equal(reasoning.event.ItemCompleted.item.text, 'Visible reasoning');
        const tool = await waitFor(() => client.frames.find(f => f.event?.ItemCompleted?.item.id === 'tool'));
        assert.match(tool.event.ItemCompleted.item.text, /pwd/);
    });
}
