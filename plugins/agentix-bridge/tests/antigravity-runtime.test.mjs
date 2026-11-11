import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AntigravityMailbox } from '../antigravity/mailbox.mjs';
import { createRuntime } from '../antigravity/runtime.mjs';
import net from 'node:net';
import { createInterface } from 'node:readline';

for (const unstarted of [false, true]) test(`first_input_handoff_flushes_original_socket_unstarted_${unstarted}`, { timeout: 10000, skip: process.platform === 'win32' }, async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-wire-'));
    const peers = [], frames = [];
    const server = net.createServer(socket => {
        peers.push(socket);
        createInterface({ input: socket }).on('line', line => {
            const frame = JSON.parse(line); frames.push({ socket, frame });
            if (frame.method === 'register') socket.write(JSON.stringify({ id: 'register', ok: true, result: {} }) + '\n');
        });
    });
    await new Promise(resolve => server.listen(join(root, 'control.sock'), resolve));
    const mailbox = new AntigravityMailbox(root, 'host');
    let sends = 0;
    const runtime = createRuntime({ root, mailbox, endpoint: `unix://${join(root, 'control.sock')}`,
        nativeNew: async () => {}, delivery: { send: async () => { sends++; } } });
    t.after(async () => {
        await runtime.close();
        peers.forEach(peer => peer.destroy());
        await new Promise(resolve => server.close(resolve));
        rmSync(root, { recursive: true, force: true });
    });
    async function waitFor(predicate) {
        for (let i = 0; i < 300; i++) {
            const result = predicate(); if (result) return result;
            await new Promise(resolve => setTimeout(resolve, 10));
        }
        throw new Error('Missing native handoff response');
    }
    const first = join(root, 'first.jsonl'), second = join(root, 'second.jsonl');
    writeFileSync(first, '');
    if (!unstarted) mailbox.publish({ hookEventName: 'SessionStart', conversationId: 'first', workspacePaths: [root], transcriptPath: first });
    await runtime.poll();
    const old = (await waitFor(() => frames.find(item => item.frame.method === 'register'))).socket;
    const send = (socket, id, method, params) => socket.write(JSON.stringify({ id, method, params }) + '\n');
    if (!unstarted) {
        send(old, 'new', 'command', { name: 'new' });
        assert.equal((await waitFor(() => frames.find(item => item.frame.id === 'new'))).frame.ok, true);
    }
    send(old, 'prompt', 'prompt', { request_id: 'request', text: 'hello' });
    await waitFor(() => sends === 1);
    writeFileSync(second, JSON.stringify({ step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'hello' }) + '\n');
    for (const hookEventName of ['SessionStart', 'PreInvocation']) mailbox.publish({ hookEventName, conversationId: 'second', workspacePaths: [root], transcriptPath: second });
    await runtime.poll();
    const response = await waitFor(() => frames.find(item => item.frame.id === 'prompt'));
    assert.equal(response.socket, old);
    assert.deepEqual(response.frame, { id: 'prompt', ok: true, result: { turn_id: 'second:step:0' } });
    const replacement = (await waitFor(() => frames.find(item => item.frame.params?.session_id === 'second'))).socket;
    send(replacement, 'retry', 'prompt', { request_id: 'request', text: 'hello' });
    assert.deepEqual((await waitFor(() => frames.find(item => item.frame.id === 'retry'))).frame.result, { turn_id: 'second:step:0' });
    assert.equal(sends, 1);
});

test('runtime_registers_native_identity_consumes_hooks_and_restores_history', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-runtime-')); t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 'transcript.jsonl');
    writeFileSync(path, JSON.stringify({ step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'hello' }) + '\n');
    const mailbox = new AntigravityMailbox(root, 'host');
    const identity = { conversationId: 'native', workspacePaths: [root], transcriptPath: path };
    for (const hookEventName of ['SessionStart', 'PreInvocation', 'Stop']) mailbox.publish({ ...identity, hookEventName, fullyIdle: true });
    const records = [], events = [];
    let handlers;
    const factory = options => { handlers = options; return { sequence: 0, open: record => records.push(record), event: event => events.push(event), close: async () => {} }; };
    const runtime = createRuntime({ root, mailbox, clientId: 'host', pid: 42, transportFactory: factory });
    await runtime.poll();
    assert.equal(records[0].agent, 'antigravity');
    assert.equal(records[0].pid, 42);
    assert.equal(records[0].session_file, path);
    assert.equal((await handlers.handle('history', {})).turns[0].status, 'completed');
    assert.equal(events.filter(e => e.TurnCompleted).length, 1);
    await runtime.close();
    const recovered = createRuntime({ root, mailbox, clientId: 'host', pid: 42, transportFactory: factory });
    await recovered.poll();
    assert.equal((await handlers.handle('history', {})).turns[0].user_text, 'hello');
    assert.equal(events.filter(e => e.TurnCompleted).length, 1);
    await recovered.close();
});

test('queued_session_switches_keep_prior_identity_and_distinct_history', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-switch-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const mailbox = new AntigravityMailbox(root, 'host');
    const records = [], events = [], handlers = [];
    for (const conversationId of ['first', 'second']) {
        const transcriptPath = join(root, `${conversationId}.jsonl`);
        writeFileSync(transcriptPath, JSON.stringify({ step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: conversationId }) + '\n');
        for (const hookEventName of ['SessionStart', 'PreInvocation', 'Stop']) {
            mailbox.publish({ conversationId, workspacePaths: [root], transcriptPath, hookEventName, fullyIdle: true });
        }
    }
    const runtime = createRuntime({ root, mailbox, clientId: 'host', transportFactory: options => {
        handlers.push(options);
        return { sequence: 0, open: record => records.push(record), event: event => events.push(event), close: async () => {} };
    } });
    await Promise.all([runtime.poll(), runtime.poll()]);
    assert.deepEqual(records.map(record => record.session_id), ['first', 'second']);
    assert.equal(records[1].previous_session_id, 'first');
    assert.equal((await handlers[1].handle('history', {})).turns[0].user_text, 'second');
    assert.deepEqual(events.filter(event => event.SessionSwitchStarted), [
        { SessionSwitchStarted: { session_id: 'first', client_id: 'host' } },
    ]);
    assert.deepEqual(events.filter(event => event.TurnCompleted).map(event => event.TurnCompleted.session_id), ['first', 'second']);
    await runtime.close();
});

test('runtime_passes_registration_configuration_to_terminal_delivery', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-configure-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const mailbox = new AntigravityMailbox(root, 'host');
    mailbox.publish({ hookEventName: 'SessionStart', conversationId: 'native', workspacePaths: [root], transcriptPath: join(root, 'missing.jsonl') });
    let configured;
    const runtime = createRuntime({ root, mailbox, delivery: { configure: value => { configured = value; } },
        transportFactory: options => ({ sequence: 0, event: () => {}, close: async () => {},
            open: () => options.registered({ multiplexer: { kind: 'tmux' } }) }) });
    await runtime.poll();
    assert.deepEqual(configured, { multiplexer: { kind: 'tmux' } });
    await runtime.close();
});

test('runtime_polls_native_approvals_after_hooks_and_replays_on_registration', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-approval-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 'transcript.jsonl');
    writeFileSync(path, JSON.stringify({ step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'hello' }) + '\n');
    const mailbox = new AntigravityMailbox(root, 'host'), events = [];
    for (const hookEventName of ['SessionStart', 'PreInvocation']) mailbox.publish({ hookEventName, conversationId: 'native', workspacePaths: [root], transcriptPath: path });
    let handlers;
    const runtime = createRuntime({ root, mailbox, interactions: { inspect: async () => ({ kind: 'command_approval', title: 'Run?', detail: 'printf', choices: ['Yes', 'No'], fingerprint: 'scope' }) },
        transportFactory: options => { handlers = options; return { sequence: 0, open: () => {}, event: value => events.push(value), close: async () => {} }; } });
    await runtime.poll();
    const pending = events.filter(event => event.InteractionRequested);
    assert.equal(pending.length, 1);
    handlers.registered({});
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(events.filter(event => event.InteractionRequested).length, 2);
    await runtime.close();
});

test('delayed_hook_consumption_preserves_each_native_turn_in_one_conversation', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-delayed-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 'transcript.jsonl'), mailbox = new AntigravityMailbox(root, 'host');
    const identity = { conversationId: 'native', workspacePaths: [root], transcriptPath: path };
    const rows = [];
    for (let step = 0; step < 2; step++) {
        rows.push(JSON.stringify({ step_index: step, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'same text' }));
        writeFileSync(path, rows.join('\n') + '\n');
        if (!step) mailbox.publish({ ...identity, hookEventName: 'SessionStart' });
        mailbox.publish({ ...identity, hookEventName: 'PreInvocation' });
        mailbox.publish({ ...identity, hookEventName: 'Stop', fullyIdle: true });
    }
    let handlers;
    const runtime = createRuntime({ root, mailbox, transportFactory: options => { handlers = options; return {
        sequence: 0, open: () => {}, event: () => {}, close: async () => {},
    }; } });
    await runtime.poll();
    const turns = (await handlers.handle('history', {})).turns;
    assert.deepEqual(turns.map(turn => turn.status), ['completed', 'completed']);
    assert.deepEqual(turns.map(turn => turn.id), ['native:step:0', 'native:step:1']);
    await runtime.close();
});

test('lazy_new_session_acknowledges_first_input_before_closing_original_transport', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-handoff-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const mailbox = new AntigravityMailbox(root, 'host'), handlers = [], closed = [], calls = [];
    const firstPath = join(root, 'first.jsonl'), secondPath = join(root, 'second.jsonl');
    writeFileSync(firstPath, '');
    mailbox.publish({ hookEventName: 'SessionStart', conversationId: 'first', workspacePaths: [root], transcriptPath: firstPath });
    const runtime = createRuntime({ root, mailbox, nativeNew: async () => { calls.push('new'); }, delivery: { send: async () => { calls.push('send'); } },
        transportFactory: options => {
            const index = handlers.length; handlers.push(options);
            return { sequence: 0, open: () => {}, event: () => {}, close: async () => { closed.push(index); } };
        } });
    await runtime.poll();
    await handlers[0].handle('command', { name: 'new' });
    const pending = handlers[0].handle('prompt', { request_id: 'first-input', text: 'hello' });
    await new Promise(resolve => setImmediate(resolve));
    writeFileSync(secondPath, JSON.stringify({ step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'hello' }) + '\n');
    for (const hookEventName of ['SessionStart', 'PreInvocation']) mailbox.publish({ hookEventName, conversationId: 'second', workspacePaths: [root], transcriptPath: secondPath });
    await runtime.poll();
    assert.deepEqual(await pending, { turn_id: 'second:step:0' });
    assert.deepEqual(await handlers[1].handle('prompt', { request_id: 'first-input', text: 'hello' }), { turn_id: 'second:step:0' });
    assert.deepEqual(calls, ['new', 'send']);
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(closed, [0]);
    await runtime.close();
});

test('empty_process_registers_once_and_keeps_identity_across_sidecar_restart', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-empty-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const mailbox = new AntigravityMailbox(root, 'empty-host');
    const records = [], handlers = [];
    const options = { root, mailbox, clientId: 'empty-host', pid: 42, cwd: root, sessionRoot: root, transportFactory: settings => {
        handlers.push(settings);
        return { sequence: 0, open: record => records.push(record), event: () => {}, close: async () => {} };
    } };
    let runtime = createRuntime(options);
    await runtime.poll(); await runtime.poll();
    assert.equal(records.length, 1);
    assert.match(records[0].session_id, /^[a-f0-9]{64}-pending$/);
    assert.ok(records[0].session_file.startsWith(join(root, '.agentix-pending')));
    assert.equal(handlers[0].snapshot().session.name, 'New session (PID 42)');
    assert.equal(records[0].cwd, root);
    assert.equal(handlers[0].snapshot().session.status, 'idle');
    assert.deepEqual((await handlers[0].handle('history')).turns, []);
    await runtime.close();
    runtime = createRuntime(options); await runtime.poll();
    assert.equal(records[1].session_id, records[0].session_id);
    await runtime.close();
    runtime = createRuntime({ ...options, clientId: 'another-process', pid: 43, mailbox: new AntigravityMailbox(root, 'another-process') });
    await runtime.poll();
    assert.notEqual(records[2].session_id.slice(0, 8), records[0].session_id.slice(0, 8));
    assert.equal(handlers[2].snapshot().session.name, 'New session (PID 43)');
    await runtime.close();
});

test('same_directory_hosts_keep_independent_bindings_across_repeated_new_and_restart', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-multiple-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const hosts = ['pane-a', 'pane-b'].map((clientId, index) => {
        const records = [], handlers = [], sends = [];
        const mailbox = new AntigravityMailbox(root, clientId);
        const options = { root, mailbox, clientId, pid: 42 + index, cwd: root, sessionRoot: root,
            nativeNew: async () => {}, delivery: { send: async text => sends.push(text) },
            transportFactory: settings => {
                handlers.push(settings);
                return { sequence: 0, open: record => records.push(record), event: () => {}, close: async () => {} };
            } };
        return { options, mailbox, records, handlers, sends, runtime: createRuntime(options) };
    });
    t.after(async () => { for (const host of hosts) await host.runtime.close(); });
    for (const host of hosts) await host.runtime.poll();
    assert.notEqual(hosts[0].records[0].session_id, hosts[1].records[0].session_id);
    for (let round = 0; round < 3; round++) {
        for (const host of hosts) {
            const oldId = host.records.at(-1).session_id;
            const handler = host.handlers.at(-1);
            if (round) await handler.handle('command', { name: 'new' });
            const text = `${host.options.clientId}-${round}`;
            const pending = handler.handle('prompt', { request_id: text, text });
            await new Promise(resolve => setImmediate(resolve));
            const transcriptPath = join(root, `${text}.jsonl`);
            writeFileSync(transcriptPath, JSON.stringify({ step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: text }) + '\n');
            for (const hookEventName of ['SessionStart', 'PreInvocation', 'Stop']) {
                host.mailbox.publish({ hookEventName, conversationId: text, workspacePaths: [root], transcriptPath, fullyIdle: true });
            }
            await host.runtime.poll();
            assert.deepEqual(await pending, { turn_id: `${text}:step:0` });
            assert.equal(host.records.at(-1).previous_session_id, oldId);
            assert.equal(host.records.at(-1).client_id, host.options.clientId);
            assert.equal(host.records.at(-1).pid, host.options.pid);
            assert.deepEqual((await host.handlers.at(-1).handle('history')).turns.map(turn => turn.user_text), [text]);
            await host.runtime.close();
            host.runtime = createRuntime(host.options);
            await host.runtime.poll();
            assert.equal(host.records.at(-1).session_id, text);
            assert.deepEqual(await host.handlers.at(-1).handle('prompt', { request_id: text, text }), { turn_id: `${text}:step:0` });
            assert.equal(host.sends.length, round + 1, 'reconnection must not resend acknowledged input');
        }
        assert.notEqual(hosts[0].records.at(-1).session_id, hosts[1].records.at(-1).session_id);
    }
});

test('runtime_drains_later_turns_after_missing_stop_and_returns_latest_history', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-missing-stop-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 'transcript.jsonl'), mailbox = new AntigravityMailbox(root, 'host');
    const identity = { conversationId: 'native', workspacePaths: [root], transcriptPath: path };
    const user = (step_index, content) => ({ step_index, content, type: 'USER_INPUT', source: 'USER_EXPLICIT' });
    const answer = (step_index, content) => ({ step_index, content, type: 'PLANNER_RESPONSE', source: 'MODEL' });
    const write = rows => writeFileSync(path, rows.map(row => JSON.stringify(row)).join('\n') + '\n');
    const hook = (hookEventName, extra = {}) => mailbox.publish({ ...identity, hookEventName, ...extra });
    let handlers;
    const runtime = createRuntime({ root, mailbox, transportFactory: options => {
        handlers = options; return { open() {}, close: async () => {}, event() {}, usage() {} };
    } });
    t.after(() => runtime.close());
    write([user(0, 'first')]); hook('SessionStart'); hook('PreInvocation'); await runtime.poll();
    // Terminal Escape may omit Stop; the next user step must still make progress.
    write([user(0, 'first'), answer(1, 'partial'), user(9, 'second')]); hook('PreInvocation');
    write([user(0, 'first'), answer(1, 'partial'), user(9, 'second'), answer(10, 'second answer')]); hook('Stop', { fullyIdle: true });
    await runtime.poll();
    const history = await handlers.handle('history', { limit: 1 });
    assert.equal(history.turns[0].id, 'native:step:9');
    assert.equal(history.turns[0].agent_text, 'second answer');
    assert.equal(history.turns[0].status, 'completed');
    assert.equal(mailbox.identity().agentix_turn_id, 'native:step:9');
    const { readdirSync } = await import('node:fs');
    assert.equal(readdirSync(mailbox.path).filter(name => name.endsWith('.event.json')).length, 0);
});
