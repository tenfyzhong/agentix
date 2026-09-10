import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { readTranscript } from '../claude/history.mjs';

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
