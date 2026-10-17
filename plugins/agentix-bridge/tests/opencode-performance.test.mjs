import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { mkdirSync, mkdtempSync, readdirSync, rmSync, statSync } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { performance } from 'node:perf_hooks';
import { promisify } from 'node:util';

// Opt-in, isolated measurements. No timing threshold depends on machine load.
// SDK responses and Taskix runner are deterministic fixtures: CLI counts are
// measured, but subprocess/provider/rendering latency is deliberately excluded.
// Phase CPU/time include the final GC; RSS is post-GC and max RSS is cumulative.
// OPENCODE_PERF_TASKIX=/absolute/path/to/taskix adds actual subprocess timings
// using isolated HOME/config/vault/SQLite and disabled external memory/routing.
// OPENCODE_PERF_TASKIX_ONLY=1 isolates that scenario's Node RSS from cache stress.
// OPENCODE_PERF_ROOT selects another checkout's plugins for a paired comparison.
// Fixtures target OpenCode 2.0.32. Historical 1.19.3 comparisons require their
// original harness; these results must not be compared as one unchanged workload.
const filename = fileURLToPath(import.meta.url);
const plugins = process.env.OPENCODE_PERF_ROOT ?? resolve(dirname(filename), '../..');
const measured = process.env.AGENTIX_OPENCODE_PERFORMANCE === '1';

async function measure(name, work) {
    if (process.env.OPENCODE_PERF_TASKIX_ONLY === '1' && !name.startsWith('actual_taskix_')) return null;
    globalThis.gc?.();
    const memory = process.memoryUsage(), cpu = process.cpuUsage(), start = performance.now();
    const result = await work();
    globalThis.gc?.();
    const used = process.cpuUsage(cpu), after = process.memoryUsage();
    return { name, elapsed_ms: +(performance.now() - start).toFixed(2), cpu_ms: +((used.user + used.system) / 1000).toFixed(2),
        rss_bytes: after.rss, rss_delta_bytes: after.rss - memory.rss, max_rss_kib: process.resourceUsage().maxRSS,
        heap_delta_bytes: after.heapUsed - memory.heapUsed, ...result };
}

function storedBytes(directory) {
    return readdirSync(directory, { withFileTypes: true }).reduce((sum, entry) => {
        const path = join(directory, entry.name);
        return sum + (entry.isDirectory() ? storedBytes(path) : statSync(path).size);
    }, 0);
}

function bridgeHost(directory, sessionID, list = async () => ({ data: [], cursor: {} })) {
    const location = { directory };
    const session = { id: sessionID, location, title: 'Performance fixture', time: { created: 1000, updated: 1000 } };
    return {
        app: { version: '2.0.32' }, location,
        ui: { router: { current: () => ({ type: 'session', sessionID }) }, model: { current: () => undefined } },
        data: { location: { default: () => location }, on: () => () => {}, listen: () => () => {},
            session: { status: () => 'idle', get: () => session } },
        client: { session: { get: async () => session }, message: { list } },
    };
}

async function taskixHost(setup, directory, options) {
    const sessionHooks = new Map(), toolHooks = new Map(), pending = [], errors = [], storage = new Map(), histories = new Map();
    let waiter, sequence = 0;
    const cleanup = await setup({
        location: { directory },
        storage: {
            get: async key => structuredClone(storage.get(key)),
            set: async (key, value) => { storage.set(key, structuredClone(value)); },
            remove: async key => { storage.delete(key); },
        },
        session: {
            get: async ({ sessionID }) => ({ id: sessionID, location: { directory } }),
            context: async ({ sessionID }) => structuredClone(histories.get(sessionID) ?? []),
            hook: async (name, callback) => { sessionHooks.set(name, callback); },
        },
        skill: { transform: async callback => callback({ add() {} }) },
        tool: {
            transform: async callback => callback({ add() {} }),
            hook: async (name, callback) => { toolHooks.set(name, callback); },
        },
        event: { async *subscribe({ signal }) {
            signal.addEventListener('abort', () => { waiter?.(undefined); waiter = undefined; }, { once: true });
            while (!signal.aborted) {
                const item = pending.shift() ?? await new Promise(resolve => { waiter = resolve; });
                if (!item) break;
                yield item.frame;
                item.done();
            }
        } },
    }, { ...options, onError: error => errors.push(error) });
    function event(type, data) {
        return new Promise(done => {
            const item = { frame: { id: 'evt_' + sequence++, type, created: Date.now(), data, location: { directory } }, done };
            if (waiter) { const ready = waiter; waiter = undefined; ready(item); }
            else pending.push(item);
        });
    }
    return {
        async prompt(sessionID, messageID, text) {
            await sessionHooks.get('prompt')({ sessionID, messageID, prompt: { text } });
            histories.set(sessionID, [...(histories.get(sessionID) ?? []), { id: messageID, type: 'user', text, time: { created: Date.now() } }]);
            await event('session.inbox.delivered', { sessionID, inboxID: messageID });
        },
        beforeTool: sessionID => toolHooks.get('execute.before')?.({ sessionID, id: 'tool_' + sequence++, input: {} }),
        event,
        async dispose() {
            await cleanup();
            assert.deepEqual(errors.map(error => error.message), [], 'native event processing must succeed');
        },
    };
}

async function workload() {
    const { OpenCodeBridge } = await import(pathToFileURL(join(plugins, 'agentix-bridge/opencode/runtime.mjs')));
    const { openQueue } = await import(pathToFileURL(join(plugins, 'agentix-bridge/opencode/state.mjs')));
    const { setupOpenCode } = await import(pathToFileURL(join(plugins, 'taskix-manager/opencode/runtime.mjs')));
    const root = mkdtempSync(join(tmpdir(), 'opencode-performance-'));
    const results = [];
    let bridge;
    try {
        results.push(await measure('bridge_12000_messages_and_text_parts', async () => {
            const api = bridgeHost(root, 'ses_perf');
            bridge = new OpenCodeBridge(api, { stateDir: root, poll: false,
                transportFactory: () => ({ sequence: 0, open() {}, async close() {}, event() {} }) });
            await bridge.start();
            await bridge.event({ type: 'session.inbox.enqueued', data: { sessionID: 'ses_perf', inboxID: 'msg_root',
                item: { id: 'msg_root', sessionID: 'ses_perf', type: 'user', time: { created: 1000 }, payload: { text: 'Inspect the repository' } } } });
            await bridge.event({ type: 'session.inbox.delivered', data: { sessionID: 'ses_perf', inboxID: 'msg_root' } });
            for (let index = 0; index < 12000; index++) {
                await bridge.event({ type: 'session.step.started', data: { assistantMessageID: 'msg_' + index, sessionID: 'ses_perf',
                    agent: 'build', model: { providerID: 'fixture', id: 'model' }, started: 1000 + index } });
                await bridge.event({ type: 'session.text.ended', data: { assistantMessageID: 'msg_' + index, sessionID: 'ses_perf', ordinal: 0, text: index + ':' + 'x'.repeat(1024) } });
            }
            return { messages: bridge.messages.size, parts: bridge.parts.size, retained_text_bytes: [...bridge.parts.values()].reduce((sum, value) => sum + (typeof value === 'string' ? Buffer.byteLength(value) : 0), 0) };
        }));
        await bridge?.close(); bridge = undefined;
        results.push(await measure('native_history_2400_tool_messages_one_complete_turn', async () => {
            let rows = [], reads = 0;
            const api = bridgeHost(root, 'ses_history', async ({ cursor, limit, order }) => {
                reads++;
                assert.ok(!(cursor && order), 'OpenCode rejects explicit order together with an opaque cursor');
                const end = cursor ? Number(cursor.slice('opaque-page-'.length)) : rows.length;
                const start = Math.max(0, end - limit);
                const data = rows.slice(start, end);
                return { data: cursor || order === 'desc' ? data.reverse() : data, cursor: data.length ? { next: 'opaque-page-' + start } : {} };
            });
            bridge = new OpenCodeBridge(api, { stateDir: root, poll: false,
                transportFactory: () => ({ sequence: 0, open() {}, async close() {}, event() {} }) });
            await bridge.start(); reads = 0;
            rows = [{ id: 'msg_0', type: 'user', time: { created: 1000 }, text: 'Inspect all tool results' }];
            for (let index = 1; index <= 2400; index++) rows.push({
                id: 'msg_' + index, type: 'assistant', agent: 'build', model: { providerID: 'fixture', id: 'model' },
                time: { created: 1000 + index, completed: 1001 + index }, finish: 'tool-calls',
                content: [{ id: 'tool_' + index, type: 'tool', name: 'read', time: { created: 1000 + index, completed: 1001 + index },
                    state: { status: 'completed', input: {}, content: [{ type: 'text', text: 'Read fixture' }] } }],
            });
            const history = await bridge.command('history', { limit: 20 });
            assert.equal(history.turns.length, 1);
            assert.equal(history.turns[0].tools.length, 2400);
            return { native_history_calls: reads, tool_items: history.turns[0].tools.length };
        }));
        await bridge?.close(); bridge = undefined;
        let file;
        const queueDirectory = join(root, 'queue');
        results.push(await measure('journal_5000_receipts_and_10000_state_updates', async () => {
            const write = fs.writeFileSync, append = fs.appendFileSync;
            let writeBytes = 0, writeCalls = 0, checkpointBytes = 0, checkpoints = 0, appending = false;
            fs.writeFileSync = (path, data, ...args) => {
                // appendFileSync internally calls writeFileSync with a descriptor;
                // count those bytes only at the append boundary.
                if (!appending) {
                    writeBytes += Buffer.byteLength(data); writeCalls++;
                    if (file && String(path).startsWith(file + '.') && !String(path).startsWith(file + '.receipts')) {
                        checkpointBytes += Buffer.byteLength(data); checkpoints++;
                    }
                }
                return write(path, data, ...args);
            };
            fs.appendFileSync = (path, data, ...args) => {
                writeBytes += Buffer.byteLength(data); writeCalls++; appending = true;
                try { return append(path, data, ...args); } finally { appending = false; }
            };
            syncBuiltinESMExports();
            try {
                const saved = openQueue(queueDirectory, 'ses_journal'); file = saved.file;
                for (let index = 0; index < 2048; index++) saved.queue.remember('request_' + index, { turn_id: 'turn_' + index }, null, { method: 'prompt', text: 'x'.repeat(1024) });
                for (let index = 0; index < 5000; index++) { saved.queue.pause(); saved.queue.resume(); }
                let rejected = 0;
                for (let index = 2048; index < 5000; index++) {
                    try { saved.queue.remember('request_' + index, { turn_id: 'turn_' + index }, null, { method: 'prompt', text: 'x'.repeat(1024) }); }
                    catch (error) { if (!/capacity|limit|full/i.test(error.message)) throw error; rejected++; }
                }
                return { journal_bytes: statSync(file).size, total_queue_storage_bytes: storedBytes(queueDirectory), additional_requests_rejected: rejected,
                    fs_write_bytes: writeBytes, fs_write_calls: writeCalls, checkpoint_write_bytes: checkpointBytes, checkpoints };
            } finally {
                fs.writeFileSync = write; fs.appendFileSync = append; syncBuiltinESMExports();
            }
        }));
        results.push(await measure('journal_restore_and_earliest_receipt', async () => {
            const restored = openQueue(queueDirectory, 'ses_journal');
            assert.deepEqual(restored.queue.receipt('request_0', { method: 'prompt', text: 'x'.repeat(1024) }), { turn_id: 'turn_0' });
            return { hot_receipts: restored.queue.state().receipts.length, journal_bytes: statSync(file).size, total_queue_storage_bytes: storedBytes(queueDirectory) };
        }));
        results.push(await measure('taskix_100_sessions_2000_tool_callbacks', async () => {
            let heartbeats = 0, shutdowns = 0, contexts = 0;
            const timers = new Map();
            const hooks = await taskixHost(setupOpenCode, root, {
                runner: async args => {
                    if (args[0] === 'hook' && args[1] === 'heartbeat') heartbeats++;
                    if (args[0] === 'hook' && args[1] === 'session-end') shutdowns++;
                    if (args[0] === 'context') contexts++;
                    return { result: {} };
                },
                routing: { env: {} },
                timers: { setInterval(fn) { const timer = {}; timers.set(timer, fn); return timer; }, clearInterval(timer) { timers.delete(timer); } },
            });
            try {
                for (let index = 0; index < 100; index++) {
                    const sessionID = 'ses_' + index;
                    await hooks.prompt(sessionID, 'msg_' + index, 'Inspect the repository');
                    for (let tool = 0; tool < 20; tool++) await hooks.beforeTool(sessionID);
                    await hooks.event('session.execution.succeeded', { sessionID });
                    await hooks.event('session.execution.succeeded', { sessionID });
                }
                const idleTimers = timers.size, idleShutdowns = shutdowns;
                await hooks.dispose();
                return { heartbeat_cli_calls: heartbeats, context_cli_calls: contexts, idle_timers: idleTimers, idle_sessions_shutdown: idleShutdowns, disposed_timers: timers.size };
            } finally { await hooks.dispose(); }
        }));
        results.push(await measure('taskix_owned_lease_survives_idle_and_session_churn', async () => {
            const timers = new Map();
            let ownedHeartbeats = 0, ownedShutdowns = 0;
            const hooks = await taskixHost(setupOpenCode, root, {
                runner: async (args, options) => {
                    const owned = options.session === 'ses_owned';
                    if (owned && args[0] === 'hook' && args[1] === 'heartbeat') ownedHeartbeats++;
                    if (owned && args[0] === 'hook' && args[1] === 'session-end') ownedShutdowns++;
                    return { result: owned ? { task_id: 'task_owned', task: { id: 'task_owned', status: 'EXECUTING' },
                        lease: { token: 'lease_owned', executor: 'agent:opencode:ses_owned', session_id: 'ses_owned' } } : {} };
                },
                routing: { env: {} },
                timers: { setInterval(fn) { const timer = {}; timers.set(timer, fn); return timer; }, clearInterval(timer) { timers.delete(timer); } },
            });
            try {
                for (const sessionID of ['ses_owned', ...Array.from({ length: 100 }, (_, index) => 'ses_churn_' + index)]) {
                    await hooks.prompt(sessionID, 'msg_' + sessionID, 'Continue tracked work');
                    await hooks.event('session.execution.succeeded', { sessionID });
                }
                // Exercise twenty renewal cycles; no wall-clock sleep is needed.
                for (let minute = 0; minute < 20; minute++) for (const callback of [...timers.values()]) await callback();
                return { owned_heartbeat_calls: ownedHeartbeats, owned_shutdowns_before_dispose: ownedShutdowns, retained_renewal_timers: timers.size };
            } finally { await hooks.dispose(); assert.equal(timers.size, 0); }
        }));
        if (process.env.OPENCODE_PERF_TASKIX) results.push(await measure('actual_taskix_cli_1_session_40_native_tool_callbacks', async () => {
            const binary = resolve(process.env.OPENCODE_PERF_TASKIX);
            const { buildArgs } = await import(pathToFileURL(join(plugins, 'taskix-manager/taskix-cli.mjs')));
            const directory = join(root, 'actual-taskix'), home = join(directory, 'home'), vault = join(directory, 'vault');
            mkdirSync(join(vault, '.obsidian'), { recursive: true }); mkdirSync(home, { recursive: true });
            const env = { PATH: '/opt/homebrew/bin:/usr/bin:/bin', HOME: home, LANG: 'C',
                XDG_CONFIG_HOME: join(home, '.config'), XDG_CACHE_HOME: join(home, '.cache'), XDG_STATE_HOME: join(home, '.local/state'),
                TASKIX_CONFIG: join(directory, 'config.toml'), TASKIX_MEMORY_ENABLED: 'false', TASKIX_JEV_ENABLED: 'false' };
            const execute = promisify(execFile);
            let calls = 0, heartbeats = 0, contexts = 0, cliElapsed = 0;
            const runner = async (args, options = {}) => {
                const start = performance.now(); calls++;
                if (args[0] === 'hook' && args[1] === 'heartbeat') heartbeats++;
                if (args[0] === 'context') contexts++;
                try {
                    const { stdout } = await execute(binary, buildArgs(args, options), {
                        cwd: directory, env, signal: options.signal, timeout: 30000, maxBuffer: 4 * 1024 * 1024,
                    });
                    const response = JSON.parse(stdout);
                    assert.equal(response.ok, true, stdout);
                    return response;
                } finally { cliElapsed += performance.now() - start; }
            };
            await runner(['init', '--root', vault, '--directory', 'Tasks', '--database', join(directory, 'tasks.sqlite3')]);
            await runner(['project', 'register', '--root', directory, '--name', 'Isolated performance fixture']);
            const timers = new Set();
            const hooks = await taskixHost(setupOpenCode, directory, {
                runner, routing: { env: { TASKIX_MEMORY_ENABLED: 'false', TASKIX_JEV_ENABLED: 'false' } },
                timers: { setInterval(fn) { timers.add(fn); return fn; }, clearInterval(fn) { timers.delete(fn); } },
            });
            try {
                await hooks.prompt('ses_actual', 'msg_actual', 'Inspect the isolated fixture');
                for (let tool = 0; tool < 40; tool++) await hooks.beforeTool('ses_actual');
                await hooks.event('session.execution.succeeded', { sessionID: 'ses_actual' });
            } finally { await hooks.dispose(); }
            assert.equal(timers.size, 0);
            return { cli_binary: binary, cli_invocations: calls, heartbeat_cli_calls: heartbeats, context_cli_calls: contexts,
                cli_total_elapsed_ms: +cliElapsed.toFixed(2), cpu_scope: 'parent_node_only_excludes_taskix_children',
                isolation: 'temporary_HOME_config_vault_SQLite_no_inherited_credentials' };
        }));
        return results.filter(Boolean);
    } finally { await bridge?.close(); rmSync(root, { recursive: true, force: true }); }
}

if (process.env.OPENCODE_PERF_CHILD === '1') {
    process.stdout.write(JSON.stringify({ fixture_contract: 'OpenCode 2.0.32 native API', node: process.version, platform: process.platform, arch: process.arch, results: await workload() }) + '\n');
} else {
    test('opencode sustained workloads retain bounded caches and replay identity', { skip: !measured }, t => {
        const child = spawnSync(process.execPath, ['--expose-gc', filename], {
            env: { ...process.env, OPENCODE_PERF_CHILD: '1' }, encoding: 'utf8', timeout: 120000, maxBuffer: 1024 * 1024,
        });
        assert.equal(child.status, 0, child.stderr);
        const report = JSON.parse(child.stdout.trim());
        t.diagnostic(JSON.stringify(report));
        if (process.env.OPENCODE_PERF_REPORT_ONLY === '1') return;
        if (process.env.OPENCODE_PERF_TASKIX_ONLY === '1') {
            assert.equal(report.results.length, 1, 'the isolated CLI scenario requires OPENCODE_PERF_TASKIX');
            assert.equal(report.results[0].heartbeat_cli_calls, 0, 'actual CLI verification must eliminate per-tool heartbeats');
            return;
        }
        const [cache, history, journal, restored, hooks, owned, actual] = report.results;
        assert.ok(cache.messages <= 4096, 'native message metadata must have a finite retention bound');
        assert.ok(cache.parts <= 8192 && cache.retained_text_bytes <= 8 * 1024 * 1024, 'text retention must have count and byte bounds');
        assert.ok(history.native_history_calls <= 20);
        assert.equal(journal.additional_requests_rejected, 0, 'bounded cache must retain access to archived receipts without a lifetime admission cap');
        assert.ok(restored.hot_receipts <= 2048, 'restart must not load every historical receipt into the hot cache');
        assert.ok(journal.journal_bytes < 4 * 1024 * 1024, 'checkpointing must bound journal replay bytes for admitted workload');
        assert.ok(journal.fs_write_bytes < 20 * 1024 * 1024, 'checkpoint maintenance must keep total writes within the sustained workload budget');
        assert.ok(journal.checkpoints < 200, 'checkpoint size must not retrigger checkpointing on every state update');
        assert.ok(hooks.heartbeat_cli_calls <= 100, 'nearby native tool callbacks must share heartbeat freshness');
        assert.ok(hooks.context_cli_calls <= 200, 'duplicate native idle events must share one ownership check per turn');
        assert.equal(hooks.idle_timers, 0, 'idle native sessions must not retain renewal timers');
        assert.ok(hooks.idle_sessions_shutdown >= 36, 'inactive session retention must be bounded across 100 sessions');
        assert.equal(hooks.disposed_timers, 0);
        assert.ok(owned.owned_heartbeat_calls >= 20, 'an owned executing Task must continue renewal during native idle');
        assert.equal(owned.owned_shutdowns_before_dispose, 0, 'session trimming must preserve ownership of active Tasks');
        assert.equal(owned.retained_renewal_timers, 1, 'only the owned lease should retain background renewal');
        if (actual) assert.equal(actual.heartbeat_cli_calls, 0, 'actual CLI verification must eliminate per-tool heartbeats');
    });
}
