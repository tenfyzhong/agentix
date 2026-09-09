// Compare serialization work for legacy snapshots and the current incremental callback.
// No model, IM request, filesystem write, or rmux process is used.
import { performance } from 'node:perf_hooks';
import { ClaudeSession } from '../claude/session.mjs';
const samples = Number(process.env.BENCH_SAMPLES ?? 3);
const turns = Number(process.env.BENCH_CLAUDE_TURNS ?? 500);
const median = values => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
const results = {};
for (const mode of ['snapshot', 'journal']) {
    const times = [], sizes = [];
    for (let sample = 0; sample < samples; sample++) {
        let bytes = 0;
        const capture = value => { bytes += Buffer.byteLength(JSON.stringify(value)); };
        const session = new ClaudeSession({ notify: async () => {},
            ...(mode === 'snapshot' ? { persist: capture } : { append: capture }) });
        const identity = { hook_event_name: 'SessionStart', session_id: 'benchmark', cwd: '/tmp', transcript_path: '/tmp/benchmark.jsonl' };
        session.hook(identity);
        const start = performance.now();
        for (let i = 0; i < turns; i++) {
            const id = 'request-' + i;
            const pending = session.request('prompt', { request_id: id, text: 'benchmark prompt' });
            session.acknowledge(id); await pending;
            session.hook({ ...identity, hook_event_name: 'Stop', last_assistant_message: 'benchmark answer '.repeat(60) });
        }
        times.push(performance.now() - start); sizes.push(bytes);
        if (session.turns.length !== turns || session.receipts.size !== turns) throw new Error('Lost state');
    }
    results[mode] = { ms: times, median_ms: median(times), serialized_bytes: sizes, median_bytes: median(sizes) };
}
console.log(JSON.stringify({ node: process.version, platform: process.platform, arch: process.arch, samples, turns, ...results }, null, 2));
