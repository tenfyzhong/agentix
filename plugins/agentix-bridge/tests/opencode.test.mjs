import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { OpenCodeBridge } from '../opencode/runtime.mjs';
import { normalizeMessages } from '../opencode/turns.mjs';

function fixture(t, options = {}) {
    const root = mkdtempSync(join(tmpdir(), 'agentix-opencode2-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const events = [],
        calls = [],
        listeners = new Map(),
        rows = new Map([
            ['ses_one', []],
            ['ses_two', []],
        ]);
    let route = { type: 'session', sessionID: 'ses_one' },
        state = 'idle';
    const session = (id) => ({
        id,
        title: 'Test',
        location: { directory: root },
        time: { updated: 1000 },
        model: { providerID: 'test', id: 'model' },
    });
    const api = {
        app: { version: '2.0.32' },
        location: { directory: root },
        ui: {
            router: {
                current: () => route,
                navigate: (value) => {
                    route = value;
                },
            },
            model: {
                current: () => ({ providerID: 'test', modelID: 'model' }),
            },
            toast: { show: (value) => calls.push(['toast', value]) },
        },
        data: {
            on: (name, fn) => {
                listeners.set(name, fn);
                return () => listeners.delete(name);
            },
            session: {
                status: () => state,
                message: {
                    get: (sid, id) => rows.get(sid)?.find((m) => m.id === id),
                },
            },
            location: { default: () => ({ directory: root }) },
        },
        client: {
            session: {
                get: async ({ sessionID }) => session(sessionID),
                create: async () => session('ses_new'),
                prompt: async (params) => {
                    calls.push(['prompt', params]);
                    state = 'running';
                    return {
                        id: params.id,
                        type: 'user',
                        payload: { text: params.text },
                    };
                },
                interrupt: async (params) => {
                    calls.push(['interrupt', params]);
                    state = 'idle';
                    return { interrupted: true };
                },
                compact: async (params) => {
                    calls.push(['compact', params]);
                    return { id: 'compact', type: 'compaction' };
                },
                update: async (params) => {
                    calls.push(['rename', params]);
                },
                switchModel: async (params) => {
                    calls.push(['model', params]);
                },
                diff: async () => [],
                message: {
                    get: async ({ sessionID, messageID }) =>
                        rows.get(sessionID)?.find((m) => m.id === messageID),
                },
                form: {
                    list: async () => [],
                    reply: async (params) => {
                        calls.push(['form', params]);
                    },
                    cancel: async (params) => {
                        calls.push(['cancel', params]);
                    },
                },
            },
            message: {
                list: async ({ sessionID, cursor, limit, order }) => {
                    assert.ok(
                        !(cursor && order),
                        'native SDK rejects cursor combined with order',
                    );
                    const all = [...(rows.get(sessionID) || [])].reverse(),
                        offset = cursor ? Number(cursor.slice(6)) : 0;
                    return {
                        data: all.slice(offset, offset + limit),
                        cursor: {
                            next:
                                'cursor' + Math.min(offset + limit, all.length),
                        },
                    };
                },
            },
            permission: {
                list: async () => [],
                reply: async (params) => {
                    calls.push(['permission', params]);
                },
            },
            model: {
                list: async () => ({
                    data: [{ id: 'model', providerID: 'test', enabled: true }],
                }),
            },
            skill: {
                list: async () => ({
                    data: [{ name: 'test-skill', description: 'Test' }],
                }),
            },
        },
    };
    let registration;
    const transport = {
        sequence: 0,
        open: (value) => {
            registration = value;
        },
        close: async () => {},
        event: (value) => events.push(value),
        usage: (id, value) => calls.push(['usage', id, value]),
    };
    const bridge = new OpenCodeBridge(api, {
        stateDir: root,
        poll: false,
        transportFactory: () => transport,
        ...options,
    });
    t.after(() => bridge.close());
    const emit = (type, data, id = 'evt_test') =>
        bridge.event({ id, type, data, created: 1000 });
    const user = async (id) => {
        await emit('session.inbox.enqueued', {
            sessionID: 'ses_one',
            inboxID: id,
            item: { type: 'user', payload: { text: 'Hello' } },
        });
        await emit('session.inbox.delivered', {
            sessionID: 'ses_one',
            inboxID: id,
        });
    };
    return {
        root,
        api,
        bridge,
        calls,
        events,
        rows,
        emit,
        user,
        listeners,
        registration: () => registration,
        idle: () => {
            state = 'idle';
        },
    };
}

test('opencode2_uses_native_router_location_and_setup_contract', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    assert.equal(f.registration().agent, 'opencode');
    assert.equal(f.registration().pid, process.pid);
    assert.equal(f.registration().cwd, f.root);
    assert.equal(f.registration().session_id, 'ses_one');
    assert.ok(f.listeners.has('session.text.delta'));
    const plugin = (await import('../opencode/tui.mjs')).default;
    assert.equal(typeof plugin.setup, 'function');
    assert.equal(plugin.tui, undefined);
});

test('opencode2_gates_verified_baseline_and_rejects_legacy_context', async (t) => {
    const f = fixture(t);
    f.api.app.version = '1.19.3';
    await assert.rejects(f.bridge.start(), /2\.0\.32/);
});

test('opencode2_prompt_admission_keeps_native_id_and_request_intent', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    const params = { request_id: 'request', text: 'Hello' };
    const first = await f.bridge.command('prompt', params);
    assert.match(first.turn_id, /^msg_/);
    assert.equal(f.calls.at(-1)[1].id, first.turn_id);
    assert.equal(f.calls.at(-1)[1].text, 'Hello');
    assert.equal(f.calls.at(-1)[1].parts, undefined);
    assert.deepEqual(await f.bridge.command('prompt', params), first);
    await assert.rejects(
        f.bridge.command('prompt', { ...params, text: 'Changed' }),
        /different request/,
    );
});

test('opencode2_streams_text_ordinal_once_and_compaction_does_not_complete_turn', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.user('u');
    await f.emit('session.step.started', {
        sessionID: 'ses_one',
        assistantMessageID: 'a',
    });
    await f.emit('session.text.delta', {
        sessionID: 'ses_one',
        assistantMessageID: 'a',
        ordinal: 0,
        delta: 'Hello ',
    });
    await f.emit('session.text.ended', {
        sessionID: 'ses_one',
        assistantMessageID: 'a',
        ordinal: 0,
        text: 'Hello world',
    });
    await f.emit('session.text.ended', {
        sessionID: 'ses_one',
        assistantMessageID: 'a',
        ordinal: 0,
        text: 'Hello world',
    });
    await f.emit('session.compaction.ended', {
        sessionID: 'ses_one',
        text: 'Hidden summary',
    });
    assert.equal(f.events.filter((e) => e.TurnCompleted).length, 0);
    assert.equal(
        f.events
            .filter((e) => e.AgentMessageDelta)
            .map((e) => e.AgentMessageDelta.delta)
            .join(''),
        'Hello world',
    );
    f.rows.set('ses_one', [
        { id: 'u', type: 'user', text: 'Hello' },
        {
            id: 'a',
            type: 'assistant',
            content: [{ type: 'text', text: 'Hello world' }],
        },
        { id: 'idle', type: 'idle', outcome: 'succeeded' },
    ]);
    await f.emit('session.execution.succeeded', { sessionID: 'ses_one' });
    assert.equal(
        f.events.find((e) => e.TurnCompleted).TurnCompleted.turn_id,
        'u',
    );
    assert.equal(
        f.events.find((e) => e.ItemCompleted?.item.kind === 'agentMessage')
            .ItemCompleted.item.text,
        'Hello world',
    );
});

test('opencode2_tagged_history_hides_compaction_and_preserves_large_text_and_files', () => {
    const text = 'x'.repeat(64 * 1024);
    const normalized = normalizeMessages([
        {
            id: 'u',
            type: 'user',
            text,
            files: [{ name: 'diagram.png', uri: 'data:xxx' }],
        },
        { id: 'c', type: 'compaction', summary: 'Private' },
        { id: 's', type: 'synthetic', text: 'Private' },
        {
            id: 'a',
            type: 'assistant',
            content: [{ type: 'text', text: 'Answer' }],
        },
        { id: 'i', type: 'idle', outcome: 'interrupted' },
    ]);
    assert.equal(normalized.turns.length, 1);
    assert.ok(normalized.turns[0].user_text.startsWith(text));
    assert.match(normalized.turns[0].user_text, /diagram.png/);
    assert.equal(normalized.turns[0].agent_text, 'Answer');
    assert.equal(normalized.turns[0].status, 'interrupted');
    assert.equal(normalized.messageTurns.get('a'), 'u');
});

test('opencode2_history_pagination_does_not_skip_native_rows_with_opaque_cursors', async (t) => {
    const f = fixture(t);
    f.rows.set(
        'ses_one',
        Array.from({ length: 150 }, (_, i) => [
            { id: 'u' + i, type: 'user', text: 'Q' + i },
            {
                id: 'a' + i,
                type: 'assistant',
                content: [{ type: 'text', text: 'A' + i }],
            },
            { id: 'i' + i, type: 'idle', outcome: 'succeeded' },
        ]).flat(),
    );
    await f.bridge.start();
    const one = await f.bridge.history({ limit: 5 });
    const two = await f.bridge.history({ limit: 5, cursor: one.older_cursor });
    assert.deepEqual(
        [...two.turns, ...one.turns].map((t) => t.user_text),
        Array.from({ length: 10 }, (_, i) => 'Q' + (140 + i)),
    );
});

test('opencode2_busy_resume_restores_assistant_identity_for_delta', async (t) => {
    const f = fixture(t);
    f.rows.set('ses_one', [
        { id: 'u', type: 'user', text: 'Hello' },
        { id: 'a', type: 'assistant', content: [] },
    ]);
    await f.bridge.start();
    await f.emit('session.text.delta', {
        sessionID: 'ses_one',
        assistantMessageID: 'a',
        ordinal: 0,
        delta: 'Resumed',
    });
    assert.equal(f.events.at(-1).AgentMessageDelta.turn_id, 'u');
});

test('opencode2_permissions_and_forms_use_native_session_field_keys_and_option_values', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.emit('permission.asked', {
        id: 'per',
        sessionID: 'ses_one',
        action: 'shell',
        resources: ['git status'],
    });
    await f.bridge.command('interaction', {
        rpc_id: 'per',
        response: { decision: 'always' },
    });
    assert.deepEqual(f.calls.at(-1), [
        'permission',
        { sessionID: 'ses_one', requestID: 'per', decision: 'always' },
    ]);
    await f.emit('form.created', {
        form: {
            id: 'frm',
            sessionID: 'ses_one',
            title: 'Choose',
            fields: [
                {
                    key: 'q0',
                    type: 'multiselect',
                    title: 'Pick',
                    custom: false,
                    options: [
                        { label: 'One', value: 'one' },
                        { label: 'Two', value: 'two' },
                    ],
                },
            ],
        },
    });
    const card = f.events.find(
        (e) => e.InteractionRequested?.method === 'opencode.form',
    ).InteractionRequested;
    assert.equal(card.payload.questions[0].id, 'q0');
    await f.bridge.command('interaction', {
        rpc_id: 'frm',
        response: { answers: { q0: { answers: ['One', 'Two'] } } },
    });
    assert.deepEqual(f.calls.at(-1), [
        'form',
        { sessionID: 'ses_one', formID: 'frm', answer: { q0: ['one', 'two'] } },
    ]);
});

test('opencode2_unsupported_typed_or_conditional_forms_explicitly_require_native_terminal', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.emit('form.created', {
        form: {
            id: 'typed',
            sessionID: 'ses_one',
            title: 'Typed',
            fields: [{ key: 'count', type: 'integer', minimum: 2 }],
        },
    });
    const request = f.events.at(-1).InteractionRequested;
    assert.match(request.detail, /terminal/i);
    await assert.rejects(
        f.bridge.command('interaction', {
            rpc_id: 'typed',
            response: { answers: { count: { answers: ['4'] } } },
        }),
        /terminal/i,
    );
    await f.bridge.command('interaction', {
        rpc_id: 'typed',
        response: { decision: 'reject' },
    });
    assert.equal(f.calls.at(-1)[0], 'cancel');
});

test('opencode2_native_controls_use_new_admission_apis_and_session_model', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.bridge.control('rename', 'New');
    await f.bridge.control('model', 'test/model');
    await f.bridge.control('compact');
    assert.ok(f.calls.some(([name]) => name === 'compact'));
    assert.ok(f.calls.some(([name]) => name === 'model'));
    assert.match((await f.bridge.control('skills')).body, /test-skill/);
    await f.bridge.command('stop');
    assert.equal(f.calls.at(-1)[0], 'interrupt');
    await f.bridge.control('new');
    await new Promise((r) => setImmediate(r));
    assert.equal(f.registration().previous_session_id, 'ses_one');
});

test('opencode2_history_deadline_aborts_and_initial_failure_remains_discoverable', async (t) => {
    const f = fixture(t, { sdkTimeoutMs: 10 });
    let signal;
    f.api.client.message.list = (_input, options) => {
        signal = options.signal;
        return new Promise(() => {});
    };
    await f.bridge.start();
    assert.equal(f.registration().session_id, 'ses_one');
    assert.equal(signal.aborted, true);
    await assert.rejects(f.bridge.history({}), /timed out/i);
});

test('opencode2_live_caches_have_no_prompt_or_attachment_payload_retention', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    for (let i = 0; i < 5000; i++)
        await f.emit('session.inbox.enqueued', {
            sessionID: 'ses_one',
            inboxID: 'u' + i,
            item: {
                type: 'user',
                payload: {
                    text: 'x'.repeat(64 * 1024),
                    files: [{ uri: 'data:' + 'A'.repeat(1024 * 1024) }],
                },
            },
        });
    assert.ok(f.bridge.pendingUsers.size <= 4096);
    assert.equal(f.bridge.userParts, undefined);
    assert.equal(f.bridge.native.prompts, undefined);
});

test('opencode2_queue_archive_preserves_receipts_and_crash_uncertainty', async (t) => {
    const { openQueue } = await import('../opencode/state.mjs');
    const root = mkdtempSync(join(tmpdir(), 'queue2-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const saved = openQueue(root, 's', undefined, {
        maxReceipts: 8,
        checkpointRecords: 4,
    });
    for (let i = 0; i < 32; i++) {
        saved.queue.remember('r' + i, { turn_id: 't' + i }, null, {
            method: 'prompt',
            text: 'Hello',
        });
        saved.queue.finish();
    }
    saved.queue.remember(
        'inflight',
        { turn_id: 't' },
        { id: 'inflight', text: 'Maybe' },
        { method: 'prompt', text: 'Maybe' },
    );
    const restored = openQueue(root, 's', undefined, { maxReceipts: 8 });
    assert.deepEqual(restored.queue.receipt('r0'), { turn_id: 't0' });
    assert.ok(restored.queue.view().uncertain);
    assert.ok(statSync(saved.file).size < 10000);
});

test('opencode2_generated_contracts_match_source', async () => {
    const { readFile } = await import('node:fs/promises');
    const original = await readFile(
        new URL('../../shared/opencode-turns.mjs', import.meta.url),
        'utf8',
    );
    for (const host of ['agentix-bridge', 'taskix-manager'])
        assert.equal(
            await readFile(
                new URL(`../../${host}/opencode/turns.mjs`, import.meta.url),
                'utf8',
            ),
            original,
        );
});

test('opencode2_shutdown_interrupt_preserves_inflight_turn_for_native_resume', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    const result = await f.bridge.command('prompt', {
        request_id: 'shutdown',
        text: 'Resume later',
    });
    await f.emit('session.execution.interrupted', {
        sessionID: 'ses_one',
        reason: 'shutdown',
    });
    assert.equal(f.events.filter((event) => event.TurnCompleted).length, 0);
    assert.equal(f.bridge.turn, result.turn_id);
    assert.ok(f.bridge.queue.state().inflight);
});

test('opencode2_duplicate_native_delivery_does_not_reset_newer_active_turn', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.user('first');
    await f.user('second');
    f.rows.set('ses_one', [{ id: 'first', type: 'user', text: 'Old' }]);
    await f.emit('session.inbox.delivered', {
        sessionID: 'ses_one',
        inboxID: 'first',
    });
    await f.emit('session.step.started', {
        sessionID: 'ses_one',
        assistantMessageID: 'latest',
    });
    await f.emit('session.text.delta', {
        sessionID: 'ses_one',
        assistantMessageID: 'latest',
        ordinal: 0,
        delta: 'Latest',
    });
    assert.equal(f.events.at(-1).AgentMessageDelta.turn_id, 'second');
});

test('opencode2_validated_string_forms_keep_constraints_in_native_terminal', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.emit('form.created', {
        form: {
            id: 'validated',
            sessionID: 'ses_one',
            title: 'Code',
            fields: [{ key: 'code', type: 'string', pattern: '^[A-Z]{4}$' }],
        },
    });
    assert.match(
        f.events.at(-1).InteractionRequested.detail,
        /native terminal/,
    );
});

test('opencode2_history_cursor_preserves_shared_execution_outcome_for_steered_subturns', async (t) => {
    const f = fixture(t);
    f.rows.set('ses_one', [
        { id: 'a', type: 'user', text: 'A' },
        {
            id: 'aa',
            type: 'assistant',
            content: [{ type: 'text', text: 'Answer A' }],
        },
        { id: 'b', type: 'user', text: 'B' },
        {
            id: 'bb',
            type: 'assistant',
            content: [{ type: 'text', text: 'Answer B' }],
        },
        { id: 'idle', type: 'idle', outcome: 'succeeded' },
    ]);
    await f.bridge.start();
    const latest = await f.bridge.history({ limit: 1 });
    const older = await f.bridge.history({
        limit: 1,
        cursor: latest.older_cursor,
    });
    assert.deepEqual(
        [...older.turns, ...latest.turns].map((turn) => [turn.id, turn.status]),
        [
            ['a', 'completed'],
            ['b', 'completed'],
        ],
    );
});

test('opencode2_session_switch_clears_model_pending_requests_and_resume_predecessor', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    const client = f.registration().client_id;
    await f.bridge.control('model', 'test/model');
    await f.emit('permission.asked', {
        id: 'old_permission',
        sessionID: 'ses_one',
        action: 'shell',
        resources: ['pwd'],
    });
    f.api.ui.router.navigate({ type: 'session', sessionID: 'ses_two' });
    await f.bridge.refresh();
    assert.equal(f.bridge.model, undefined);
    assert.equal(f.registration().client_id, client);
    assert.equal(f.registration().previous_session_id, undefined);
    await assert.rejects(
        f.bridge.command('interaction', {
            rpc_id: 'old_permission',
            response: { decision: 'once' },
        }),
        /pending/,
    );
});

test('opencode2_resumed_text_ordinal_is_independent_from_reasoning_and_tool_content', async (t) => {
    const f = fixture(t);
    f.rows.set('ses_one', [
        { id: 'u', type: 'user', text: 'Hello' },
        {
            id: 'a',
            type: 'assistant',
            content: [
                { type: 'reasoning', text: 'Private' },
                { type: 'text', text: 'Known' },
            ],
        },
    ]);
    await f.bridge.start();
    await f.emit('session.text.ended', {
        sessionID: 'ses_one',
        assistantMessageID: 'a',
        ordinal: 0,
        text: 'Known',
    });
    assert.equal(f.events.filter((event) => event.AgentMessageDelta).length, 0);
});

test('opencode2_rejection_feedback_denies_permission_without_hijacking_real_shutdown', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.user('reject_turn');
    await f.emit('session.step.started', {
        sessionID: 'ses_one',
        assistantMessageID: 'reject_assistant',
    });
    for (const decision of ['once', 'always', 'reject']) {
        const id = 'permission_' + decision;
        await f.emit('permission.asked', {
            id,
            sessionID: 'ses_one',
            action: 'shell',
            resources: ['pwd'],
            source: { type: 'tool', messageID: 'reject_assistant', id: 'tool' },
        });
        await f.bridge.command('interaction', {
            rpc_id: id,
            response: { decision },
        });
        assert.deepEqual(f.calls.at(-1), [
            'permission',
            {
                sessionID: 'ses_one',
                requestID: id,
                decision,
                ...(decision === 'reject'
                    ? { message: 'Rejected by user from Agentix.' }
                    : {}),
            },
        ]);
    }
    await f.emit('permission.replied', {
        sessionID: 'ses_one',
        requestID: 'permission_reject',
        reply: 'reject',
    });
    await f.emit('session.execution.interrupted', {
        sessionID: 'ses_one',
        reason: 'shutdown',
    });
    assert.equal(f.events.filter((event) => event.TurnCompleted).length, 0);
    assert.equal(f.bridge.turn, 'reject_turn');
});

test('opencode2_rejected_supported_and_native_only_forms_include_native_feedback', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    for (const field of [
        { key: 'q0', type: 'string' },
        { key: 'n', type: 'integer' },
    ]) {
        const id = 'form_' + field.key;
        await f.emit('form.created', {
            form: {
                id,
                sessionID: 'ses_one',
                title: 'Question',
                fields: [field],
            },
        });
        await f.bridge.command('interaction', {
            rpc_id: id,
            response: { decision: 'reject' },
        });
        assert.deepEqual(f.calls.at(-1), [
            'cancel',
            {
                sessionID: 'ses_one',
                formID: id,
                message: 'Rejected by user from Agentix.',
            },
        ]);
    }
});

test('opencode2_history_read_does_not_consume_a_live_user_delivery_or_text', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    f.rows.set('ses_one', [
        { id: 'new_user', type: 'user', text: 'Live prompt' },
        {
            id: 'new_assistant',
            type: 'assistant',
            content: [{ type: 'text', text: 'Live response' }],
        },
    ]);
    await f.bridge.history({});
    await f.user('new_user');
    await f.emit('session.step.started', {
        sessionID: 'ses_one',
        assistantMessageID: 'new_assistant',
    });
    await f.emit('session.text.ended', {
        sessionID: 'ses_one',
        assistantMessageID: 'new_assistant',
        ordinal: 0,
        text: 'Live response',
    });
    assert.equal(
        f.events.filter((e) => e.TurnStarted?.turn_id === 'new_user').length,
        1,
    );
    assert.equal(
        f.events
            .filter((e) => e.AgentMessageDelta)
            .map((e) => e.AgentMessageDelta.delta)
            .join(''),
        'Live response',
    );
});

test('opencode2_interaction_replay_cannot_leak_previous_session_requests_after_switch', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    let release;
    f.api.client.permission.list = () =>
        new Promise((resolve) => {
            release = resolve;
        });
    const replay = f.bridge.replayInteractions();
    f.api.ui.router.navigate({ type: 'session', sessionID: 'ses_two' });
    await f.bridge.refresh();
    release([
        {
            id: 'old_request',
            sessionID: 'ses_one',
            action: 'shell',
            resources: ['private old command'],
        },
    ]);
    await replay;
    assert.equal(f.bridge.pending.has('old_request'), false);
    assert.equal(
        f.events.some((e) => e.InteractionRequested?.rpc_id === 'old_request'),
        false,
    );
});

test('opencode2_slow_previous_route_cannot_replace_newer_session_registration', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    const get = f.api.client.session.get;
    let release;
    f.api.client.session.get = (input) =>
        input.sessionID === 'ses_two'
            ? new Promise((resolve) => {
                  release = () => get(input).then(resolve);
              })
            : get(input);
    f.api.ui.router.navigate({ type: 'session', sessionID: 'ses_two' });
    const older = f.bridge.refresh();
    f.api.ui.router.navigate({ type: 'session', sessionID: 'ses_three' });
    await f.bridge.refresh();
    await release();
    await older;
    assert.equal(f.registration().session_id, 'ses_three');
    assert.equal(f.bridge.session.id, 'ses_three');
});

test('opencode2_other_session_events_do_not_enter_the_native_dispatch_backlog', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    let calls = 0;
    const event = f.bridge.event.bind(f.bridge);
    f.bridge.event = (frame) => {
        calls++;
        return event(frame);
    };
    const dispatch = f.listeners.get('session.text.ended');
    for (let i = 0; i < 100; i++)
        dispatch({
            type: 'session.text.ended',
            data: {
                sessionID: 'ses_two',
                assistantMessageID: 'other',
                ordinal: i,
                text: 'x'.repeat(1024 * 1024),
            },
        });
    await f.bridge.events;
    assert.equal(
        calls,
        0,
        'foreign event payloads must be filtered before cloning and queuing',
    );
});

test('opencode2_busy_reconnect_restores_a_native_turn_during_recoverable_step_failure', async (t) => {
    const f = fixture(t);
    f.api.data.session.status = () => 'running';
    f.rows.set('ses_one', [
        { id: 'retry_user', type: 'user', text: 'Retry' },
        {
            id: 'failed_step',
            type: 'assistant',
            error: { type: 'provider.rate_limit', message: 'Retrying' },
            content: [],
        },
    ]);
    await f.bridge.start();
    assert.equal(f.bridge.turn, 'retry_user');
    await f.emit('session.step.started', {
        sessionID: 'ses_one',
        assistantMessageID: 'retry_step',
    });
    await f.emit('session.text.delta', {
        sessionID: 'ses_one',
        assistantMessageID: 'retry_step',
        ordinal: 0,
        delta: 'Recovered',
    });
    assert.equal(f.events.at(-1).AgentMessageDelta.turn_id, 'retry_user');
    f.rows.get('ses_one').push(
        {
            id: 'retry_step',
            type: 'assistant',
            content: [{ type: 'text', text: 'Recovered' }],
        },
        { id: 'retry_idle', type: 'idle', outcome: 'succeeded' },
    );
    await f.emit('session.execution.succeeded', { sessionID: 'ses_one' });
    assert.equal(
        f.events.find((e) => e.TurnCompleted).TurnCompleted.status,
        'completed',
    );
});

test('opencode2_version_gate_rejects_prerelease_and_malformed_versions', async (t) => {
    for (const version of [
        '2.0.32-dev',
        '2.0.32.4',
        'garbagev2.0.32',
        'v2.0.32',
        '2.0',
        '1.99.99',
        '2.0.31',
        '18446744073709551616.0.0',
    ]) {
        const f = fixture(t);
        f.api.app.version = version;
        await assert.rejects(
            f.bridge.start(),
            /requires OpenCode 2\.0\.32/,
            version,
        );
    }
});

test('opencode2_version_gate_accepts_stable_host_and_cli_version_formats', async (t) => {
    for (const version of ['2.0.32', 'opencode v2.0.32', ' 2.1.0\n', '3.0.0']) {
        const f = fixture(t);
        f.api.app.version = version;
        await f.bridge.start();
        assert.equal(f.registration().session_id, 'ses_one', version);
    }
});

test('opencode2_form_representability_preserves_hidden_defaults_and_terminal_fallback', async (t) => {
    const cases = [
        { name: 'plain', field: { type: 'string' }, supported: true },
        {
            name: 'conditional',
            field: {
                type: 'string',
                when: [{ field: 'other', equals: 'yes' }],
            },
        },
        {
            name: 'hidden_default',
            field: {
                type: 'integer',
                hidden: true,
                required: true,
                default: 7,
            },
            supported: true,
            answer: { value: 7 },
        },
        {
            name: 'hidden_required',
            field: { type: 'integer', hidden: true, required: true },
        },
        {
            name: 'hidden_optional',
            field: { type: 'integer', hidden: true },
            supported: true,
            answer: {},
        },
        {
            name: 'duplicate_labels',
            field: {
                type: 'string',
                options: [
                    { label: 'Same', value: 'a' },
                    { label: 'Same', value: 'b' },
                ],
            },
        },
        { name: 'minimum_items', field: { type: 'multiselect', minItems: 1 } },
        { name: 'maximum_items', field: { type: 'multiselect', maxItems: 2 } },
    ];
    const f = fixture(t);
    await f.bridge.start();
    for (const row of cases) {
        const field = { key: 'value', ...row.field };
        await f.emit('form.created', {
            form: {
                id: row.name,
                sessionID: 'ses_one',
                title: row.name,
                fields: [field],
            },
        });
        const card = f.events.at(-1).InteractionRequested;
        assert.equal(
            f.bridge.pending.get(row.name).supported,
            Boolean(row.supported),
            row.name,
        );
        if (row.supported) {
            assert.equal(
                card.payload.questions.length,
                field.hidden ? 0 : 1,
                row.name,
            );
            await f.bridge.command('interaction', {
                rpc_id: row.name,
                response: { answers: { value: { answers: ['Response'] } } },
            });
            assert.deepEqual(
                f.calls.at(-1),
                [
                    'form',
                    {
                        sessionID: 'ses_one',
                        formID: row.name,
                        answer: row.answer ?? { value: 'Response' },
                    },
                ],
                row.name,
            );
        } else {
            assert.match(card.detail, /native terminal/, row.name);
            await assert.rejects(
                f.bridge.command('interaction', {
                    rpc_id: row.name,
                    response: { answers: { value: { answers: ['Response'] } } },
                }),
                /terminal/,
                row.name,
            );
            await f.bridge.command('interaction', {
                rpc_id: row.name,
                response: { decision: 'reject' },
            });
            assert.deepEqual(
                f.calls.at(-1),
                [
                    'cancel',
                    {
                        sessionID: 'ses_one',
                        formID: row.name,
                        message: 'Rejected by user from Agentix.',
                    },
                ],
                row.name,
            );
        }
    }
});

test('opencode2_tool_dispatch_snapshots_only_consumed_fields_without_reading_large_payloads', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.user('tool_user');
    await f.emit('session.step.started', {
        sessionID: 'ses_one',
        assistantMessageID: 'tool_assistant',
    });
    const snapshots = [];
    const dispatch = f.bridge.event.bind(f.bridge);
    f.bridge.event = (snapshot) => {
        snapshots.push(snapshot);
        return dispatch(snapshot);
    };
    for (const type of [
        'session.tool.input.started',
        'session.tool.called',
        'session.tool.success',
        'session.tool.failed',
    ]) {
        const data = {
            sessionID: 'ses_one',
            assistantMessageID: 'tool_assistant',
            id: type,
            name: 'shell',
            metadata: { title: 'Original title' },
        };
        for (const key of ['input', 'output', 'error']) {
            Object.defineProperty(data, key, {
                enumerable: true,
                get() {
                    throw new Error('Unused tool payload was read: ' + key);
                },
            });
        }
        Object.defineProperty(data.metadata, 'unused', {
            enumerable: true,
            get() {
                throw new Error('Unused tool metadata was read');
            },
        });
        f.listeners.get(type)({ type, data });
        data.name = 'mutated';
        data.metadata.title = 'mutated';
    }
    await f.bridge.events;
    assert.equal(snapshots.length, 4);
    for (const snapshot of snapshots) {
        assert.deepEqual(Object.keys(snapshot.data).sort(), [
            'assistantMessageID',
            'id',
            'metadata',
            'name',
            'sessionID',
        ]);
        assert.equal(snapshot.data.name, 'shell');
        assert.deepEqual(snapshot.data.metadata, { title: 'Original title' });
    }
    assert.equal(
        f.events.filter((event) => event.ItemStarted?.kind === 'tool').length,
        2,
    );
    assert.equal(
        f.events.filter((event) => event.ItemCompleted?.item.kind === 'tool')
            .length,
        2,
    );
});

for (const evict of ['bytes', 'count']) {
    test(`opencode2_evicted_text_${evict}_does_not_replay_streamed_prefix`, async (t) => {
        const f = fixture(t);
        await f.bridge.start();
        await f.user('u');
        await f.emit('session.step.started', { sessionID: 'ses_one', assistantMessageID: 'a' });
        const text = evict === 'bytes' ? 'x'.repeat(8 * 1024 * 1024 + 1) : 'prefix';
        const data = { sessionID: 'ses_one', assistantMessageID: 'a', ordinal: 0 };
        await f.emit('session.text.delta', { ...data, delta: text });
        if (evict === 'count')
            for (let i = 0; i < 8192; i++) f.bridge.parts.set('tool:' + i, true);
        await f.emit('session.text.delta', { ...data, delta: 'next' });
        await f.emit('session.text.ended', { ...data, text: text + 'next-final' });
        await f.emit('session.text.ended', { ...data, text: text + 'next-final' });
        assert.equal(f.events.filter(e => e.AgentMessageDelta).map(e => e.AgentMessageDelta.delta).join(''), text + 'next-final');
        assert.ok(f.bridge.parts.bytes <= 8 * 1024 * 1024);
    });
}

test('opencode2_forms_preserve_required_and_allow_optional_omission', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.emit('form.created', { form: { id: 'constraints', sessionID: 'ses_one', title: 'Form', fields: [
        { key: 'required', type: 'multiselect', required: true, options: [{label:'A',value:'a'}] },
        { key: 'optional', type: 'string', required: false },
    ] } });
    const questions = f.events.at(-1).InteractionRequested.payload.questions;
    assert.equal(questions[0].required, true);
    assert.equal(questions[1].required, false);
    await assert.rejects(f.bridge.command('interaction', { rpc_id:'constraints', response:{ answers:{ required:{answers:[]} } } }), /required/i);
    assert.ok(f.bridge.pending.has('constraints'));
    await f.bridge.command('interaction', { rpc_id:'constraints', response:{ answers:{ required:{answers:['A']}, optional:{answers:[]} } } });
    assert.deepEqual(f.calls.at(-1)[1].answer, { required:['a'] });
});

test('opencode2_text_offset_cache_is_bounded_and_does_not_replay_unknown_evicted_parts', async (t) => {
    const f = fixture(t);
    await f.bridge.start();
    await f.user('u');
    await f.emit('session.step.started', { sessionID: 'ses_one', assistantMessageID: 'a' });
    const data = { sessionID:'ses_one', assistantMessageID:'a', ordinal:0 };
    await f.emit('session.text.delta', { ...data, delta:'already streamed' });
    for (let i = 0; i < 8192; i++) f.bridge.parts.set('text:' + i, 'x');
    await f.emit('session.text.started', data);
    await f.emit('session.text.ended', { ...data, text:'already streamed' });
    assert.equal(f.events.filter(e => e.AgentMessageDelta).map(e => e.AgentMessageDelta.delta).join(''), 'already streamed');
    assert.ok(f.bridge.parts.textLengths.size <= 8192);
});


test('OpenCode forwards native step usage and ignores foreign sessions and absent fields', async t => {
    const f = fixture(t);
    await f.bridge.start();
    assert.ok(f.listeners.has('session.step.ended'));
    assert.ok(f.listeners.has('session.step.failed'));
    f.listeners.get('session.step.ended')({ type: 'session.step.ended', data: { sessionID: 'ses_one', assistantMessageID: 'a', tokens: { input: 17, output: 4, cache: { read: 9 } } } });
    f.listeners.get('session.step.failed')({ type: 'session.step.failed', data: { sessionID: 'ses_one', assistantMessageID: 'b', tokens: { output: 2 } } });
    f.listeners.get('session.step.ended')({ type: 'session.step.ended', data: { sessionID: 'ses_two', assistantMessageID: 'foreign', tokens: { input: 999 } } });
    await f.bridge.events;
    assert.deepEqual(f.calls.filter(c => c[0] === 'usage'), [
        ['usage', 'a', { input: 17, output: 4, cached: 9 }], ['usage', 'b', { output: 2 }],
    ]);
});
