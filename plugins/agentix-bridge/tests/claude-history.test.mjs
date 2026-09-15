import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, appendFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { readTranscript, createTranscriptReader as latestReader } from '../claude/history.mjs';

test('Claude transcript history groups user turns and tool results without inventing user prompts', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-history-')); t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl');
    const entries = [
        { type: 'user', uuid: 'u1', sessionId: 's', message: { role: 'user', content: 'hello' } },
        { type: 'assistant', uuid: 'a1', sessionId: 's', message: { role: 'assistant', content: [{ type: 'text', text: 'checking' }, { type: 'tool_use', id: 'tool', name: 'Read', input: {} }] } },
        { type: 'user', uuid: 'r1', sessionId: 's', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'tool', content: 'contents' }] } },
        { type: 'assistant', uuid: 'a2', sessionId: 's', message: { role: 'assistant', content: [{ type: 'text', text: 'done' }] } },
        { type: 'user', uuid: 'foreign', sessionId: 'other', message: { role: 'user', content: 'wrong session' } },
    ];
    writeFileSync(path, entries.map(JSON.stringify).join('\n') + '\n{"partial":');
    const turns = readTranscript(path, 's');
    assert.equal(turns.length, 1);
    assert.equal(turns[0].id, 'u1');
    assert.equal(turns[0].agent_text, 'checking\ndone');
    assert.equal(turns[0].tools[0].status, 'completed');
});

test('Claude transcript import waits for a terminating newline before accepting the final record', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-history-tail-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl');
    const line = JSON.stringify({ type: 'user', uuid: 'u', sessionId: 's', message: { content: 'hello' } });
    writeFileSync(path, line);
    assert.deepEqual(readTranscript(path, 's'), []);
    writeFileSync(path, line + '\n');
    assert.equal(readTranscript(path, 's').length, 1);
});

test('Claude history retains visible reasoning and tool inputs and outputs as items', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-process-history-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl');
    const entries = [
        { type: 'user', uuid: 'u', sessionId: 's', message: { content: 'hello' } },
        { type: 'assistant', uuid: 'a', sessionId: 's', message: { content: [{ type: 'thinking', thinking: 'Visible reasoning' }, { type: 'tool_use', id: 't', name: 'Read', input: { file_path: '/tmp/a' } }] } },
        { type: 'user', uuid: 'r', sessionId: 's', message: { content: [{ type: 'tool_result', tool_use_id: 't', content: 'contents' }] } },
    ];
    writeFileSync(path, entries.map(JSON.stringify).join('\n') + '\n');
    const items = readTranscript(path, 's')[0].items;
    assert.equal(items.find(i => i.kind === 'reasoning')?.text, 'Visible reasoning');
    assert.match(items.find(i => i.id === 't')?.text ?? '', /file_path.*\/tmp\/a/);
    assert.match(items.find(i => i.id === 't')?.text ?? '', /contents/);
});

test('Claude completion transcript cost across long histories', { skip: !process.env.AGENTIX_HISTORY_BENCH }, t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-history-bench-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    for (const count of [100, 10000]) {
        const path = join(root, `${count}.jsonl`);
        const entries = [];
        for (let i = 0; i < count; i++) {
            entries.push({ type: 'user', uuid: `u${i}`, sessionId: 's', message: { content: `prompt ${i}` } });
            entries.push({ type: 'assistant', uuid: `a${i}`, sessionId: 's', message: { content: [{ type: 'text', text: 'x'.repeat(512) }], stop_reason: 'end_turn' } });
        }
        const data = entries.map(JSON.stringify).join('\n') + '\n';
        writeFileSync(path, data);
        const samples = [], cachedSamples = [], latest = latestReader();
        const coldStart = performance.now();
        latest(path, 's');
        const coldMs = performance.now() - coldStart;
        for (let run = 0; run < 7; run++) {
            const index = count + run;
            appendFileSync(path, jsonl([
                record(`u${index}`, 'user', `prompt ${index}`),
                record(`a${index}`, 'assistant', [{ type: 'text', text: 'x'.repeat(512) }]),
            ]));
            const start = performance.now();
            const full = readTranscript(path, 's').at(-1);
            samples.push(performance.now() - start);
            const cachedStart = performance.now();
            const cached = latest(path, 's');
            cachedSamples.push(performance.now() - cachedStart);
            assert.deepEqual(cached, full);
            assert.equal(cached.user_text, `prompt ${index}`);
            assert.equal(cached.agent_text, 'x'.repeat(512));
        }
        samples.sort((a, b) => a - b);
        cachedSamples.sort((a, b) => a - b);
        t.diagnostic(JSON.stringify({ turns: count, bytes: Buffer.byteLength(data), full_median_ms: samples[3], cached_append_median_ms: cachedSamples[3], cold_ms: coldMs }));
    }
});

const record = (uuid, type, content, extra = {}) => ({ uuid, type, sessionId: 's', message: { content }, ...extra });
const jsonl = entries => entries.map(JSON.stringify).join('\n') + '\n';

test('Claude completion reader parses only newly appended complete records', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-history-incremental-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl'), latest = latestReader();
    writeFileSync(path, jsonl(Array.from({ length: 100 }, (_, i) => record(`u${i}`, 'user', `prompt ${i}`))));
    assert.equal(latest(path, 's').user_text, 'prompt 99');
    const parse = t.mock.method(JSON, 'parse');
    latest(path, 's');
    assert.equal(parse.mock.callCount(), 0, 'unchanged history must not be parsed again');
    appendFileSync(path, jsonl([record('a', 'assistant', [{ type: 'text', text: 'answer' }])]));
    assert.equal(latest(path, 's').agent_text, 'answer');
    assert.equal(parse.mock.callCount(), 1, 'only the new record should be parsed');
});

test('Claude completion reader preserves partial UTF-8, filtering, duplicates and snapshots', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-history-partials-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl'), latest = latestReader();
    writeFileSync(path, jsonl([record('u', 'user', 'question')]));
    const first = latest(path, 's');
    first.user_text = 'mutated caller snapshot';
    const answer = Buffer.from(jsonl([record('a', 'assistant', [{ type: 'thinking', thinking: 'reason' }, { type: 'text', text: '雪🙂' }])]));
    const split = answer.indexOf(Buffer.from('🙂')) + 2;
    appendFileSync(path, answer.subarray(0, split));
    assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
    appendFileSync(path, answer.subarray(split));
    assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
    appendFileSync(path, jsonl([
        record('a', 'assistant', 'duplicate'),
        record('foreign', 'user', 'foreign', { sessionId: 'other' }),
        record('side', 'user', 'sidechain', { isSidechain: true }),
        record('meta', 'user', 'metadata', { isMeta: true }),
        record('channel', 'user', '<channel context>'),
        record('b', 'assistant', 'final'),
    ]));
    assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
    assert.equal(first.agent_text, '', 'later reads must not mutate earlier snapshots');
});

test('Claude completion reader rebuilds after rewrite, truncation, deletion and identity change', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-history-rewrite-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl'), latest = latestReader();
    for (const entries of [
        [record('u', 'user', 'initial')],
        [record('u', 'user', 'changed'), record('a', 'assistant', 'longer replacement')],
        [record('n', 'user', 'short')],
    ]) {
        writeFileSync(path, jsonl(entries));
        assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
    }
    assert.equal(latest(path, 'other'), undefined);
    assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
    rmSync(path);
    assert.equal(latest(path, 's'), undefined);
    writeFileSync(path, jsonl([record('n', 'user', 'recreated')]));
    assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
});

test('Claude completion reader matches bounded history when the tail window moves', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-history-window-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl'), latest = latestReader();
    writeFileSync(path, ' '.repeat(16 * 1024 * 1024) + '\n' + jsonl([record('u', 'user', 'tail prompt')]));
    assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
    appendFileSync(path, jsonl([record('a', 'assistant', 'tail answer')]));
    assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
});

test('Claude completion reader discards partial projection after a parse failure', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-history-failure-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl'), latest = latestReader();
    const first = record('u', 'user', 'prompt');
    writeFileSync(path, jsonl([first]));
    latest(path, 's');
    appendFileSync(path, jsonl([record('a', 'assistant', 'before failure'), record('bad', 'user', {})]));
    assert.throws(() => latest(path, 's'));
    writeFileSync(path, jsonl([first, record('a', 'assistant', 'corrected answer'), record('bad', 'user', '')]));
    assert.deepEqual(latest(path, 's'), readTranscript(path, 's').at(-1));
});

test('Claude completion hooks reuse the session reader and retain reasoning and final output', async t => {
    const { ClaudeSession } = await import('../claude/session.mjs');
    const root = mkdtempSync(join(tmpdir(), 'ax-history-hooks-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const path = join(root, 's.jsonl'), events = [];
    const session = new ClaudeSession({ event: event => events.push(event) });
    session.hook({ hook_event_name: 'SessionStart', session_id: 's', cwd: root, transcript_path: path });
    writeFileSync(path, '');
    const parse = t.mock.method(JSON, 'parse');
    for (let index = 0; index < 2; index++) {
        const prompt = `prompt ${index}`, answer = `answer ${index}`, reasoning = `reasoning ${index}`;
        appendFileSync(path, jsonl([
            record(`u${index}`, 'user', prompt),
            record(`a${index}`, 'assistant', [{ type: 'thinking', thinking: reasoning }, { type: 'text', text: answer }]),
        ]));
        const before = parse.mock.callCount();
        session.hook({ hook_event_name: 'UserPromptSubmit', session_id: 's', prompt });
        session.hook({ hook_event_name: 'Stop', session_id: 's', last_assistant_message: answer });
        assert.equal(parse.mock.callCount() - before, 2);
        assert.ok(events.some(event => event.ItemCompleted?.item.text === reasoning));
        assert.ok(events.some(event => event.ItemCompleted?.item.text === answer));
    }
    assert.equal(events.filter(event => event.TurnCompleted).length, 2);
});
