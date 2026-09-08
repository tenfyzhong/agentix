// Reusable benchmark. No model, provider, IM, or real Agentix service is contacted.
import { performance } from 'node:perf_hooks';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionState } from '../session.mjs';
import { DurableQueue } from '../queue.mjs';
import { registerBridge } from '../runtime.mjs';
import { hostFixture, listen, waitFor } from '../tests/support.mjs';
const samples = Number(process.env.BENCH_SAMPLES ?? 3);
const historyIterations = Number(process.env.BENCH_HISTORY_ITERATIONS ?? 200);
const queueItems = Number(process.env.BENCH_QUEUE_ITEMS ?? 500);
const median = values => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
const host = hostFixture();
for (let i = 0; i < 20000; i++) host.entries.push(
    { id: `user-${i}`, type: 'message', message: { role: 'user', content: 'question '.repeat(8) } },
    { id: `answer-${i}`, type: 'message', message: { role: 'assistant', content: 'answer '.repeat(40), stopReason: 'stop' } },
);
host.ctx.sessionManager.getLeafId = () => host.entries.at(-1)?.id;
const session = new SessionState(host.ctx);
const historyTimes = [], queueTimes = [], queueBytes = [], stopTimes = [];
for (let sample = 0; sample < samples; sample++) {
    const begin = performance.now();
    let count = 0;
    for (let i = 0; i < historyIterations; i++) count += session.history().turns.length;
    if (count !== historyIterations * 20) throw new Error('Invalid history benchmark output');
    historyTimes.push((performance.now() - begin) / historyIterations);
    let bytes = 0;
    const queue = new DurableQueue({}, state => { bytes += Buffer.byteLength(JSON.stringify(state)); });
    const queuedAt = performance.now();
    for (let i = 0; i < queueItems; i++) {
        queue.enqueue(`request-${i}`, 'queued benchmark prompt'); queue.claim(); queue.finish();
    }
    queueTimes.push(performance.now() - queuedAt); queueBytes.push(bytes);
    if (queue.state().receipts.length !== queueItems || queue.state().queue.length) throw new Error('Queue lost durable state');
}
if (process.platform !== 'win32') {
    const cleanup = [];
    const directory = await mkdtemp(join(tmpdir(), 'ax-bench-'));
    try {
        const server = await listen(directory, { after: fn => cleanup.push(fn) });
        const slowHost = hostFixture();
        let entered = false;
        slowHost.ctx.modelRegistry.getAvailable = async () => {
            entered = true; await new Promise(resolve => setTimeout(resolve, 100)); return [];
        };
        const bridge = registerBridge(slowHost.api, 'pi', { endpoint: `unix://${join(directory, 'control.sock')}` });
        cleanup.push(() => bridge.close());
        await slowHost.emit('session_start');
        const client = await server.next();
        for (let sample = 0; sample < samples; sample++) {
            entered = false;
            const command = client.request('command', { name: 'model' });
            await waitFor(() => entered);
            const begin = performance.now();
            await client.request('stop');
            stopTimes.push(performance.now() - begin);
            await command;
        }
    } finally {
        for (const close of cleanup.reverse()) await close();
        await rm(directory, { recursive: true, force: true });
    }
}
console.log(JSON.stringify({ node: process.version, platform: process.platform, arch: process.arch, samples,
    history: { entries: host.entries.length, iterations: historyIterations, ms_per_page: historyTimes, median_ms: median(historyTimes) },
    queue: { items: queueItems, ms: queueTimes, median_ms: median(queueTimes), persisted_bytes: queueBytes, median_bytes: median(queueBytes) },
    stop_during_100ms_command: { ms: stopTimes, median_ms: median(stopTimes) },
}, null, 2));
