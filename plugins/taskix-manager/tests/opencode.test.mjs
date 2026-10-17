import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFile } from 'node:fs/promises';
import { setupOpenCode } from '../opencode/runtime.mjs';

function fixture(options = {}) {
    const hooks = new Map(),
        tools = new Map(),
        skills = new Map(),
        calls = [],
        captures = [],
        timers = new Set(),
        errors = [];
    const queue = [],
        projection = [],
        store = options.store ?? new Map();
    let wake,
        contextReads = 0,
        yielded = 0;
    const ctx = {
        location: { directory: '/work' },
        storage: {
            get: async (key) => structuredClone(store.get(key)),
            set: async (key, value) => {
                store.set(key, structuredClone(value));
            },
            remove: async (key) => {
                store.delete(key);
            },
        },
        session: {
            async context() {
                contextReads++;
                return projection;
            },
            async get() {
                return { location: { directory: '/work' } };
            },
            async hook(name, callback) {
                hooks.set(name, callback);
                return { dispose: async () => hooks.delete(name) };
            },
        },
        skill: {
            async transform(edit) {
                edit({ add: (skill) => skills.set(skill.name, skill) });
            },
        },
        tool: {
            async transform(edit) {
                edit({ add: (tool) => tools.set(tool.name, tool) });
                return { dispose: async () => tools.clear() };
            },
        },
        event: {
            async *subscribe({ signal }) {
                signal.addEventListener('abort', () => wake?.(), {
                    once: true,
                });
                while (!signal.aborted) {
                    if (!queue.length)
                        await new Promise((resolve) => {
                            wake = resolve;
                        });
                    const item = queue.shift();
                    if (!item) continue;
                    try {
                        yielded++;
                        yield item.event;
                    } finally {
                        item.done();
                    }
                }
            },
        },
    };
    const runtimeOptions = {
        maxConcurrentEvents: 1,
        ...options,
        runner: async (args, opts) => {
            calls.push({ args, options: opts });
            if (args[0] === 'hook' && args[1] === 'record')
                captures.push(
                    JSON.parse(
                        await readFile(
                            args[args.indexOf('--file') + 1],
                            'utf8',
                        ),
                    ),
                );
            return options.runner ? options.runner(args, opts) : { result: {} };
        },
        timers: {
            setInterval(fn) {
                timers.add(fn);
                return fn;
            },
            clearInterval(fn) {
                timers.delete(fn);
            },
        },
        onError: (error) => errors.push(error),
    };
    const emit = (type, data) =>
        new Promise((done) => {
            queue.push({ event: { type, data }, done });
            wake?.();
        });
    return {
        ctx,
        calls,
        captures,
        timers,
        hooks,
        tools,
        skills,
        errors,
        projection,
        store,
        contextReads: () => contextReads,
        yielded: () => yielded,
        start: () => setupOpenCode(ctx, runtimeOptions),
        prompt: async (
            sessionID,
            messageID,
            prompt = { text: 'Implement a feature' },
        ) => {
            await hooks.get('prompt')({
                sessionID,
                messageID,
                prompt,
                delivery: 'steer',
            });
            await emit('session.inbox.delivered', {
                sessionID,
                inboxID: messageID,
            });
        },
        event: emit,
    };
}

test('opencode2 registers prompt context and native JSON schema tool with scoped identity', async () => {
    const f = fixture(),
        cleanup = await f.start();
    for (const id of ['ses_a', 'ses_b']) {
        await f.prompt(id, 'msg_' + id);
        const request = {
            sessionID: id,
            system: [],
            options: {},
            messages: [],
        };
        await f.hooks.get('context')(request);
        assert.ok(
            request.system.every(
                (part) => part.type === 'text' && typeof part.text === 'string',
            ),
        );
        assert.match(
            request.system.map((p) => p.text).join(''),
            new RegExp('Task session: ' + id),
        );
    }
    assert.equal(f.timers.size, 2);
    const tool = f.tools.get('taskix');
    assert.deepEqual(tool.input.required, ['args']);
    const signal = new AbortController().signal;
    const result = await tool.execute(
        { args: ['job', 'create', '--title', 'Example'] },
        { sessionID: 'ses_a', messageID: 'msg_ses_a', id: 'call_1', signal },
    );
    assert.equal(typeof result.content, 'string');
    assert.equal(f.calls.at(-1).options.executor, 'agent:opencode:ses_a');
    assert.equal(
        f.calls.at(-1).options.idempotencyKey,
        'opencode:ses_a:call_1',
    );
    await assert.rejects(
        tool.execute({ args: [] }, { sessionID: 'ses_a', signal }),
        /call ID/,
    );
    await cleanup();
    assert.equal(f.timers.size, 0);
    assert.equal(
        f.calls.filter((c) => c.args.join(' ') === 'hook session-end').length,
        2,
    );
});

test('opencode2 native execution events capture text once and release interrupted lease', async () => {
    const f = fixture(),
        cleanup = await f.start();
    await f.prompt('s', 'msg_user', { text: 'Hello' });
    await f.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_asst',
    });
    await f.event('session.text.delta', {
        sessionID: 's',
        assistantMessageID: 'msg_asst',
        ordinal: 0,
        delta: 'Par',
    });
    await f.event('session.text.ended', {
        sessionID: 's',
        assistantMessageID: 'msg_asst',
        ordinal: 0,
        text: 'Partial',
    });
    await f.event('session.execution.interrupted', {
        sessionID: 's',
        reason: 'user',
    });
    assert.ok(f.calls.some((c) => c.args.join(' ') === 'hook interrupt'));
    assert.ok(
        f.captures.some(
            (c) =>
                c.turn_id === 'msg_user' &&
                c.messages.some(
                    (m) =>
                        m.id === 'msg_user:msg_user:assistant' &&
                        m.text === 'Partial',
                ),
        ),
    );
    assert.equal(f.timers.size, 0);
    await cleanup();
});

test('opencode2 overlapping prompts and compaction keep independent native transcript identities', async () => {
    const f = fixture(),
        cleanup = await f.start();
    await f.prompt('s', 'msg_a', { text: 'Same' });
    await f.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_aa',
    });
    await f.prompt('s', 'msg_b', { text: 'Same' });
    await f.event('session.text.ended', {
        sessionID: 's',
        assistantMessageID: 'msg_aa',
        ordinal: 0,
        text: 'Final A',
    });
    await f.event('session.compaction.started', {
        sessionID: 's',
        reason: 'auto',
        inputID: 'msg_b',
    });
    await f.event('session.compaction.ended', {
        sessionID: 's',
        text: 'Private summary',
    });
    await f.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_bb',
    });
    await f.event('session.text.ended', {
        sessionID: 's',
        assistantMessageID: 'msg_bb',
        ordinal: 0,
        text: 'Final B',
    });
    await f.event('session.execution.succeeded', { sessionID: 's' });
    for (const [id, text] of [
        ['msg_a', 'Final A'],
        ['msg_b', 'Final B'],
    ])
        assert.ok(
            f.captures.some(
                (c) =>
                    c.turn_id === id &&
                    c.messages.some(
                        (m) => m.role === 'assistant' && m.text === text,
                    ),
            ),
        );
    assert.ok(!JSON.stringify(f.captures).includes('Private summary'));
    const contexts = f.calls.filter((c) => c.args[0] === 'context').length;
    await f.event('session.execution.succeeded', { sessionID: 's' });
    await f.event('session.status', {
        sessionID: 's',
        status: { type: 'idle' },
    });
    assert.equal(
        f.calls.filter((c) => c.args[0] === 'context').length,
        contexts,
    );
    await cleanup();
});

test('opencode2 attachment-only prompt routes without per-tool subprocess and idle timer leak', async () => {
    const f = fixture(),
        cleanup = await f.start();
    await f.prompt('s', 'msg_file', {
        text: '',
        files: [{ uri: 'file:///tmp/diagram.png', name: 'diagram.png' }],
    });
    assert.ok(
        f.captures.some((c) =>
            c.messages.some(
                (m) => m.role === 'user' && m.text.includes('diagram.png'),
            ),
        ),
    );
    assert.equal(f.timers.size, 1);
    await f.event('session.execution.succeeded', { sessionID: 's' });
    assert.equal(f.timers.size, 0);
    assert.equal(
        f.calls.filter((c) => c.args.join(' ') === 'hook heartbeat').length,
        0,
    );
    await cleanup();
});

test('opencode2 idle owned timer stops after external lease release', async () => {
    let owned = true;
    const f = fixture({
            runner: async (args) => ({
                result:
                    args[0] === 'context' && owned
                        ? { lease: { token: 'owned' } }
                        : {},
            }),
        }),
        cleanup = await f.start();
    await f.prompt('s', 'msg_user');
    await f.event('session.execution.succeeded', { sessionID: 's' });
    assert.equal(f.timers.size, 1);
    owned = false;
    await [...f.timers][0]();
    assert.equal(f.timers.size, 0);
    await cleanup();
});

test('opencode2 cleanup stops renewal without recording an active reply as terminal', async () => {
    let failing = false;
    const f = fixture({
            runner: async (args) => {
                if (failing && args[0] === 'hook' && args[1] === 'record')
                    throw new Error('capture unavailable');
                return { result: {} };
            },
        }),
        cleanup = await f.start();
    await f.prompt('s', 'msg_user');
    failing = true;
    await cleanup();
    assert.equal(f.timers.size, 0);
    assert.ok(f.calls.some((c) => c.args.join(' ') === 'hook session-end'));
});

test('opencode2 registers the bundled skill with its real reference base path', async () => {
    const f = fixture(),
        cleanup = await f.start();
    const skill = f.skills.get('taskix-manager');
    assert.ok(skill);
    assert.match(skill.path, /skills[\\/]taskix-manager[\\/]SKILL\.md$/);
    assert.match(skill.content, /references\/commands\.md/);
    assert.match(skill.description, /Manage Taskix/);
    await cleanup();
});

test('opencode2 queued admission cannot replace the delivered turn context or transcript', async () => {
    const f = fixture(),
        cleanup = await f.start();
    await f.prompt('s', 'msg_a', { text: 'Work on A' });
    await f.event('session.inbox.delivered', {
        sessionID: 's',
        inboxID: 'msg_a',
    });
    await f.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_a1',
    });
    const original = { sessionID: 's', system: [] };
    await f.hooks.get('context')(original);
    await f.hooks.get('prompt')({
        sessionID: 's',
        messageID: 'msg_b',
        prompt: { text: 'Work on B' },
        delivery: 'queue',
    });
    await f.event('session.inbox.enqueued', {
        sessionID: 's',
        inboxID: 'msg_b',
        item: {
            type: 'user',
            payload: { text: 'Work on B' },
            delivery: 'queue',
        },
    });
    const requestA = { sessionID: 's', system: [] };
    await f.hooks.get('context')(requestA);
    assert.deepEqual(
        requestA.system,
        original.system,
        'Queued B does not replace A model context',
    );
    await f.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_a2',
    });
    await f.event('session.text.ended', {
        sessionID: 's',
        assistantMessageID: 'msg_a2',
        ordinal: 0,
        text: 'Completed A',
    });
    await f.event('session.execution.succeeded', { sessionID: 's' });
    assert.ok(
        f.captures.some(
            (c) =>
                c.turn_id === 'msg_a' &&
                c.messages.some((m) => m.text === 'Completed A'),
        ),
    );
    assert.ok(!f.captures.some((c) => c.turn_id === 'msg_b'));
    await f.event('session.inbox.delivered', {
        sessionID: 's',
        inboxID: 'msg_b',
    });
    const requestB = { sessionID: 's', system: [] };
    await f.hooks.get('context')(requestB);
    assert.notDeepEqual(requestB.system, original.system);
    await f.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_b1',
    });
    await f.event('session.text.ended', {
        sessionID: 's',
        assistantMessageID: 'msg_b1',
        ordinal: 0,
        text: 'Completed B',
    });
    await f.event('session.execution.succeeded', { sessionID: 's' });
    assert.ok(
        f.captures.some(
            (c) =>
                c.turn_id === 'msg_b' &&
                c.messages.some((m) => m.text === 'Completed B'),
        ),
    );
    await cleanup();
});

test('opencode2 projected delivered users reconcile an event lag without repeating ordinary SDK reads', async () => {
    const f = fixture(),
        cleanup = await f.start();
    await f.hooks.get('prompt')({
        sessionID: 's',
        messageID: 'msg_a',
        prompt: { text: 'Delivered A' },
        delivery: 'steer',
    });
    f.projection.push({ type: 'user', id: 'msg_a', text: 'Delivered A' });
    const context = () =>
        f.hooks.get('context')({ sessionID: 's', system: [] });
    await context();
    assert.equal(f.contextReads(), 1);
    for (let step = 0; step < 10; step++) await context();
    assert.equal(
        f.contextReads(),
        1,
        'No SDK history scan on normal tool/model steps',
    );
    assert.ok(f.captures.some((c) => c.turn_id === 'msg_a'));
    await cleanup();
});

test('opencode2 restores active source after compaction without rerouting the prompt', async () => {
    const first = fixture(),
        stop = await first.start();
    await first.prompt('s', 'msg_original', { text: 'Original work' });
    await first.event('session.compaction.ended', {
        sessionID: 's',
        text: 'Private compacted state',
    });
    const durable = new Map(structuredClone([...first.store]));
    await stop();
    const resumed = fixture({ store: durable }),
        cleanup = await resumed.start();
    resumed.projection.push({
        type: 'compaction',
        id: 'msg_compact',
        summary: 'Private summary',
    });
    const request = { sessionID: 's', system: [] };
    await resumed.hooks.get('context')(request);
    assert.ok(
        request.system.some((part) => part.text.includes('msg_original')),
    );
    assert.equal(
        resumed.captures.length,
        0,
        'Restoring source does not replay prompt admission',
    );
    await resumed.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_resumed',
    });
    await resumed.event('session.text.ended', {
        sessionID: 's',
        assistantMessageID: 'msg_resumed',
        ordinal: 0,
        text: 'Recovered answer',
    });
    await resumed.event('session.execution.succeeded', { sessionID: 's' });
    assert.ok(
        resumed.captures.some(
            (c) =>
                c.turn_id === 'msg_original' &&
                c.messages.some((m) => m.text === 'Recovered answer'),
        ),
    );
    assert.equal(
        durable.size,
        0,
        'Terminal completion clears persisted active ownership',
    );
    await cleanup();
});

test('opencode2 evicted queued payload is recovered by native identity at delivery', async () => {
    const f = fixture({ maxPendingPrompts: 1, maxPendingBytes: 64 }),
        cleanup = await f.start();
    for (const id of ['msg_first', 'msg_second'])
        await f.hooks.get('prompt')({
            sessionID: 's',
            messageID: id,
            prompt: { text: id },
            delivery: 'queue',
        });
    f.projection.push({ type: 'user', id: 'msg_first', text: 'msg_first' });
    await f.event('session.inbox.delivered', {
        sessionID: 's',
        inboxID: 'msg_first',
    });
    assert.equal(
        f.contextReads(),
        1,
        'Evicted payload is read from native delivered projection',
    );
    assert.ok(f.captures.some((c) => c.turn_id === 'msg_first'));
    await cleanup();
});

test('opencode2 speculative queued sessions remain bounded without renewal timers', async () => {
    const f = fixture({ maxIdleSessions: 2 }),
        cleanup = await f.start();
    for (let i = 0; i < 5; i++)
        await f.hooks.get('prompt')({
            sessionID: 's' + i,
            messageID: 'msg_' + i,
            prompt: { text: 'Queued' },
            delivery: 'queue',
        });
    assert.equal(f.timers.size, 0);
    assert.equal(
        f.calls.filter((c) => c.args.join(' ') === 'hook session-end').length,
        3,
    );
    await cleanup();
});

test('opencode2 recovery storage bounds prompt and context payloads while retaining source identity', async () => {
    const f = fixture({
            maxPersistedPromptBytes: 3,
            maxPersistedContextBytes: 1,
        }),
        cleanup = await f.start();
    await f.prompt('s', 'msg_large', { text: 'Large original prompt' });
    const saved = f.store.get('active:s');
    assert.deepEqual(saved.turns, [{ id: 'msg_large' }]);
    assert.ok(saved.context.includes('msg_large'));
    assert.ok(!JSON.stringify(saved).includes('Large original prompt'));
    await cleanup();
});

for (const owned of [false, true])
    test(
        'opencode2 shared restoration preserves current turn without replay or lease claim: ' +
            owned,
        async () => {
            const { registerExtension } = await import('../runtime.mjs');
            const handlers = new Map(),
                registry = new Map(),
                timers = new Set(),
                calls = [],
                captures = [];
            registerExtension(
                { on: (name, fn) => handlers.set(name, fn), registerTool() {} },
                'opencode',
                async (args, opts) => {
                    calls.push(args);
                    if (args[0] === 'hook' && args[1] === 'record')
                        captures.push(
                            JSON.parse(
                                await readFile(
                                    args[args.indexOf('--file') + 1],
                                    'utf8',
                                ),
                            ),
                        );
                    return {
                        result:
                            args[0] === 'context' && owned
                                ? { lease: { token: 'actual-owned' } }
                                : {},
                    };
                },
                {
                    setInterval(fn) {
                        timers.add(fn);
                        return fn;
                    },
                    clearInterval(fn) {
                        timers.delete(fn);
                    },
                },
                undefined,
                { turnRegistry: registry },
            );
            const ctx = {
                cwd: '/work',
                sessionManager: { getSessionId: () => 'restored' },
            };
            await handlers.get('session_start')({}, ctx);
            await handlers.get('agent_restored')(
                { turnId: 'msg_original' },
                ctx,
            );
            assert.equal(registry.get('restored').id, 'msg_original');
            assert.equal(
                captures.length,
                0,
                'Restore does not replay admission',
            );
            assert.equal(timers.size, owned ? 1 : 0);
            assert.ok(
                !calls.some(
                    (args) => args.includes('claim') || args.includes('reopen'),
                ),
            );
            await handlers.get('agent_end')(
                {
                    messages: [
                        {
                            role: 'assistant',
                            content: [
                                { type: 'text', text: 'Recovered answer' },
                            ],
                        },
                    ],
                },
                ctx,
            );
            assert.equal(captures.at(-1).turn_id, 'msg_original');
            assert.deepEqual(captures.at(-1).messages, [
                {
                    id: 'msg_original:msg_original:assistant',
                    role: 'assistant',
                    text: 'Recovered answer',
                },
            ]);
            await handlers.get('session_shutdown')({}, ctx);
            assert.equal(timers.size, 0);
        },
    );

test('opencode2 restored execution starts renewal only after a new successful structured claim', async () => {
    const first = fixture(),
        stop = await first.start();
    await first.prompt('s', 'msg_original');
    const store = new Map(structuredClone([...first.store]));
    await stop();
    let owned = false;
    const f = fixture({
            store,
            runner: async (args) => {
                if (args[0] === 'task' && args[1] === 'claim') owned = true;
                return {
                    result: owned ? { lease: { token: 'new-owned' } } : {},
                };
            },
        }),
        cleanup = await f.start();
    await f.hooks.get('context')({ sessionID: 's', system: [] });
    assert.equal(f.timers.size, 0);
    await f.tools.get('taskix').execute(
        { args: ['task', 'claim', 'task_new'] },
        {
            sessionID: 's',
            id: 'call_claim',
            signal: new AbortController().signal,
        },
    );
    assert.equal(f.timers.size, 1);
    await cleanup();
});

test('opencode2 Taskix uses direct native calls so CodeMode cannot alias independent write identities', async () => {
    const f = fixture(),
        cleanup = await f.start();
    await f.prompt('s', 'msg_user');
    const tool = f.tools.get('taskix');
    assert.equal(
        tool.options?.codemode,
        false,
        'Taskix must have a native call ID for each write, rather than a shared CodeMode parent call',
    );
    for (const [id, title] of [
        ['direct_a', 'First'],
        ['direct_b', 'Second'],
        ['direct_a', 'First'],
    ])
        await tool.execute(
            { args: ['job', 'create', '--title', title] },
            {
                sessionID: 's',
                messageID: 'msg_user',
                id,
                signal: new AbortController().signal,
            },
        );
    const writes = f.calls.filter(
        (call) => call.args[0] === 'job' && call.args[1] === 'create',
    );
    assert.deepEqual(
        writes.map((call) => call.options.idempotencyKey),
        ['opencode:s:direct_a', 'opencode:s:direct_b', 'opencode:s:direct_a'],
    );
    await cleanup();
});

test('opencode2 retries transient delivered prompt routing before exposing model context', async () => {
    let fail = true;
    const f = fixture({
            runner: async (args) => {
                if (fail && args[0] === 'hook' && args[1] === 'record') {
                    fail = false;
                    throw new Error('record unavailable');
                }
                return { result: {} };
            },
        }),
        cleanup = await f.start();
    await f.prompt('s', 'msg_retry', { text: 'Retry routing' });
    f.projection.push({ type: 'user', id: 'msg_retry', text: 'Retry routing' });
    const context = { sessionID: 's', system: [] };
    await f.hooks.get('context')(context);
    assert.ok(context.system.some((part) => part.text.includes('msg_retry')));
    assert.equal(
        f.calls.filter((c) => c.args[0] === 'hook' && c.args[1] === 'record')
            .length,
        2,
    );
    await cleanup();
});

test('opencode2 retries active source persistence before exposing model context', async () => {
    const f = fixture(),
        cleanup = await f.start();
    const write = f.ctx.storage.set;
    let fail = true;
    f.ctx.storage.set = async (...args) => {
        if (fail) {
            fail = false;
            throw new Error('storage unavailable');
        }
        return write(...args);
    };
    await f.prompt('s', 'msg_retry', { text: 'Persist source' });
    f.projection.push({
        type: 'user',
        id: 'msg_retry',
        text: 'Persist source',
    });
    await f.hooks.get('context')({ sessionID: 's', system: [] });
    assert.ok(f.store.has('active:s'));
    await cleanup();
});

test('opencode2 native shutdown retains active ownership through plugin cleanup for continuation', async () => {
    const f = fixture(),
        cleanup = await f.start();
    await f.prompt('s', 'msg_resume', { text: 'Continue after shutdown' });
    await f.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_partial',
    });
    await f.event('session.text.ended', {
        sessionID: 's',
        assistantMessageID: 'msg_partial',
        ordinal: 0,
        text: 'Partial',
    });
    await f.event('session.execution.interrupted', {
        sessionID: 's',
        reason: 'shutdown',
    });
    assert.ok(
        f.store.has('active:s'),
        'Native shutdown is a continuation boundary',
    );
    assert.ok(!f.calls.some((c) => c.args.join(' ') === 'hook interrupt'));
    await cleanup();
    assert.ok(
        f.store.has('active:s'),
        'Plugin cleanup must preserve the continuation source',
    );
    assert.equal(f.timers.size, 0);
    assert.ok(
        !f.captures.some((c) =>
            c.messages.some((message) => message.role === 'assistant'),
        ),
        'Partial shutdown output is not recorded as a completed reply',
    );
});

async function eventually(predicate, message) {
    const deadline = Date.now() + 1000;
    while (Date.now() < deadline) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 5));
    }
    assert.ok(predicate(), message);
}

test('opencode2 default event pool lets an independent session progress past a blocked route', async () => {
    let release,
        fast = false;
    const blocked = new Promise((resolve) => {
        release = resolve;
    });
    const f = fixture({
            maxConcurrentEvents: undefined,
            runner: async (args, opts) => {
                if (args[0] === 'hook' && args[1] === 'record') {
                    if (opts.session === 'slow') await blocked;
                    if (opts.session === 'fast') fast = true;
                }
                return { result: {} };
            },
        }),
        cleanup = await f.start();
    await f.hooks.get('prompt')({
        sessionID: 'slow',
        messageID: 'msg_slow',
        prompt: { text: 'Slow' },
        delivery: 'steer',
    });
    await f.hooks.get('prompt')({
        sessionID: 'fast',
        messageID: 'msg_fast',
        prompt: { text: 'Fast' },
        delivery: 'steer',
    });
    const slow = f.event('session.inbox.delivered', {
        sessionID: 'slow',
        inboxID: 'msg_slow',
    });
    const other = f.event('session.inbox.delivered', {
        sessionID: 'fast',
        inboxID: 'msg_fast',
    });
    try {
        await eventually(
            () => fast,
            'Independent session is not blocked behind another session CLI',
        );
    } finally {
        release();
        await Promise.all([slow, other]);
        await cleanup();
    }
});

test('opencode2 default event pool bounds a blocked session backlog and applies backpressure', async () => {
    let release,
        started = false;
    const blocked = new Promise((resolve) => {
        release = resolve;
    });
    const f = fixture({
            maxConcurrentEvents: undefined,
            runner: async (args) => {
                if (args[0] === 'hook' && args[1] === 'record') {
                    started = true;
                    await blocked;
                }
                return { result: {} };
            },
        }),
        cleanup = await f.start();
    await f.hooks.get('prompt')({
        sessionID: 'slow',
        messageID: 'msg_slow',
        prompt: { text: 'Slow' },
        delivery: 'steer',
    });
    const sends = [
        f.event('session.inbox.delivered', {
            sessionID: 'slow',
            inboxID: 'msg_slow',
        }),
    ];
    for (let i = 0; i < 9; i++)
        sends.push(f.event('session.execution.started', { sessionID: 'slow' }));
    try {
        await eventually(
            () => started && f.yielded() >= 5,
            'Default four-event session capacity is filled',
        );
        assert.equal(
            f.yielded(),
            5,
            'Only four admitted events and one native frame waiting for capacity',
        );
    } finally {
        release();
        await Promise.all(sends);
        await cleanup();
    }
});

test('opencode2 default event pool bounds global work and cleanup joins admitted events', async () => {
    let release,
        running = 0,
        peak = 0;
    const blocked = new Promise((resolve) => {
        release = resolve;
    });
    const f = fixture({
            maxConcurrentEvents: undefined,
            runner: async (args) => {
                if (args[0] === 'hook' && args[1] === 'record') {
                    running++;
                    peak = Math.max(peak, running);
                    try {
                        await blocked;
                    } finally {
                        running--;
                    }
                }
                return { result: {} };
            },
        }),
        cleanup = await f.start();
    for (let i = 0; i < 20; i++)
        await f.hooks.get('prompt')({
            sessionID: 's' + i,
            messageID: 'msg_' + i,
            prompt: { text: 'Work' },
            delivery: 'steer',
        });
    const sends = [];
    for (let i = 0; i < 20; i++)
        sends.push(
            f.event('session.inbox.delivered', {
                sessionID: 's' + i,
                inboxID: 'msg_' + i,
            }),
        );
    try {
        await eventually(
            () => running === 16,
            'Sixteen independent events are admitted',
        );
        assert.equal(peak, 16);
        assert.equal(f.yielded(), 17);
    } finally {
        release();
        await Promise.all(sends);
        await cleanup();
    }
    assert.equal(running, 0);
    assert.equal(
        f.store.size,
        20,
        'All admitted source writes finish before cleanup releases their sessions',
    );
    assert.equal(f.timers.size, 0);
});

test('opencode2 default parallel event pool preserves order within each native session', async () => {
    const f = fixture({ maxConcurrentEvents: undefined }),
        cleanup = await f.start();
    await f.prompt('s', 'msg_user', { text: 'Ordered turn' });
    await f.event('session.step.started', {
        sessionID: 's',
        assistantMessageID: 'msg_answer',
    });
    await f.event('session.text.delta', {
        sessionID: 's',
        assistantMessageID: 'msg_answer',
        ordinal: 0,
        delta: 'One',
    });
    await f.event('session.text.delta', {
        sessionID: 's',
        assistantMessageID: 'msg_answer',
        ordinal: 0,
        delta: 'Two',
    });
    await f.event('session.execution.succeeded', { sessionID: 's' });
    await eventually(
        () =>
            f.captures.some((c) => c.messages.some((m) => m.text === 'OneTwo')),
        'Same-session frames are captured in native order',
    );
    await cleanup();
});

test('opencode2 externally released idle lease no longer pins an otherwise evictable wrapper', async () => {
    let owned = true;
    const f = fixture({
            maxIdleSessions: 0,
            runner: async (args, opts) => ({
                result:
                    args[0] === 'context' && opts.session === 'owned' && owned
                        ? { lease: { token: 'lease' } }
                        : {},
            }),
        }),
        cleanup = await f.start();
    await f.prompt('owned', 'msg_owned');
    await f.event('session.execution.succeeded', { sessionID: 'owned' });
    owned = false;
    await [...f.timers][0]();
    assert.equal(f.timers.size, 0);
    await f.hooks.get('prompt')({
        sessionID: 'other',
        messageID: 'msg_other',
        prompt: { text: 'Queued' },
        delivery: 'queue',
    });
    assert.ok(
        f.calls.some(
            (call) =>
                call.args.join(' ') === 'hook session-end' &&
                call.options.session === 'owned',
        ),
    );
    await cleanup();
});

test('opencode2 idle eviction cannot close a session while its admitted route is running', async () => {
    let release,
        started = false;
    const gate = new Promise((resolve) => {
        release = resolve;
    });
    const f = fixture({
            maxConcurrentEvents: undefined,
            maxIdleSessions: 0,
            runner: async (args, opts) => {
                if (
                    args[0] === 'hook' &&
                    args[1] === 'record' &&
                    opts.session === 'routing'
                ) {
                    started = true;
                    await gate;
                }
                return { result: {} };
            },
        }),
        cleanup = await f.start();
    await f.hooks.get('prompt')({
        sessionID: 'routing',
        messageID: 'msg_route',
        prompt: { text: 'Work' },
        delivery: 'steer',
    });
    const admitted = f.event('session.inbox.delivered', {
        sessionID: 'routing',
        inboxID: 'msg_route',
    });
    try {
        await eventually(
            () => started,
            'Route has entered its asynchronous record operation',
        );
        await f.hooks.get('prompt')({
            sessionID: 'other',
            messageID: 'msg_other',
            prompt: { text: 'Queued' },
            delivery: 'queue',
        });
        assert.ok(
            !f.calls.some(
                (c) =>
                    c.args.join(' ') === 'hook session-end' &&
                    c.options.session === 'routing',
            ),
        );
    } finally {
        release();
        await admitted;
        await cleanup();
    }
});

test('opencode2 cleanup aborts a capacity wait without admitting the held native frame', async () => {
    let release,
        started = false;
    const blocked = new Promise((resolve) => {
        release = resolve;
    });
    const f = fixture({
            maxConcurrentEvents: 4,
            runner: async (args) => {
                if (args[0] === 'hook' && args[1] === 'record' && !started) {
                    started = true;
                    await blocked;
                }
                return { result: {} };
            },
        }),
        cleanup = await f.start();
    for (let i = 0; i < 5; i++)
        await f.hooks.get('prompt')({
            sessionID: 's',
            messageID: 'msg_' + i,
            prompt: { text: 'Work ' + i },
            delivery: 'steer',
        });
    for (let i = 0; i < 5; i++)
        void f.event('session.inbox.delivered', {
            sessionID: 's',
            inboxID: 'msg_' + i,
        });
    await eventually(
        () => started && f.yielded() === 5,
        'Four events accepted and one frame waits for capacity',
    );
    const closing = cleanup();
    await new Promise((resolve) => setImmediate(resolve));
    release();
    await closing;
    assert.deepEqual(
        f.store.get('active:s').turns.map((turn) => turn.id),
        ['msg_0', 'msg_1', 'msg_2', 'msg_3'],
    );
    assert.equal(f.timers.size, 0);
});
