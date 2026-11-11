import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, appendFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { createTranscriptReader } from '../antigravity/history.mjs';

// Isolated transcript projection measurements; excludes model, terminal and network latency.
// Report timings without load-dependent thresholds. Unit tests enforce no idle content reads.
test('antigravity_transcript_projection_performance', { skip: process.env.AGENTIX_ANTIGRAVITY_PERFORMANCE !== '1' }, t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-performance-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 'transcript.jsonl'), rows = [];
    const row = (index, type, content) => JSON.stringify({ step_index: index, type,
        source: type === 'USER_INPUT' ? 'USER_EXPLICIT' : 'MODEL', content }) + '\n';
    for (let i = 0; i < 10000; i++) {
        rows.push(row(i * 2, 'USER_INPUT', `Prompt ${i}`), row(i * 2 + 1, 'PLANNER_RESPONSE', 'x'.repeat(1024)));
    }
    writeFileSync(path, rows.join(''));
    const read = createTranscriptReader();
    function measure(name, count, action) {
        const cpu = process.cpuUsage(), start = performance.now();
        for (let i = 0; i < count; i++) action(i);
        const elapsed = performance.now() - start, used = process.cpuUsage(cpu);
        t.diagnostic(JSON.stringify({ name, count, elapsed_ms: +elapsed.toFixed(2), per_operation_ms: +(elapsed / count).toFixed(3),
            cpu_ms: (used.user + used.system) / 1000, rss_bytes: process.memoryUsage().rss, transcript_bytes: statSync(path).size }));
    }
    measure('cold_projection', 1, () => assert.equal(read(path, 'native').length, 200));
    measure('unchanged_poll', 1000, () => assert.equal(read(path, 'native').at(-1).user_text, 'Prompt 9999'));
    measure('append_projection', 20, i => {
        appendFileSync(path, row(20000 + i * 2, 'USER_INPUT', `New ${i}`) + row(20001 + i * 2, 'PLANNER_RESPONSE', 'answer'));
        const turns = read(path, 'native');
        assert.equal(turns.length, 200);
        assert.equal(turns.at(-1).agent_text, 'answer');
        assert.equal(turns.at(-1).user_text, `New ${i}`);
    });
});
