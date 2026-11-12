import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, appendFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createTranscriptReader } from '../antigravity/history.mjs';
import { AntigravitySession } from '../antigravity/session.mjs';

const row = (step_index, type, content, extra = {}) => JSON.stringify({ step_index, type,
    source: type === 'USER_INPUT' ? 'USER_EXPLICIT' : 'MODEL', status: 'DONE', content, ...extra }) + '\n';
function fixture(t) {
    const root = mkdtempSync(join(tmpdir(), 'antigravity-history-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    return join(root, 'transcript.jsonl');
}
test('native_steps_preserve_distinct_identical_prompts_and_exclude_thinking', t => {
    const path = fixture(t);
    writeFileSync(path, row(0, 'USER_INPUT', 'hello') + row(1, 'PLANNER_RESPONSE', 'answer', { thinking: 'private' })
        + row(2, 'CHECKPOINT', 'internal', { source: 'SYSTEM' }) + row(3, 'GENERIC', 'raw tool output') + row(4, 'USER_INPUT', 'hello'));
    const read = createTranscriptReader();
    const turns = read(path, 'conversation');
    assert.equal(turns.length, 2);
    assert.notEqual(turns[0].id, turns[1].id);
    assert.equal(turns[0].agent_text, 'answer');
    assert.equal(turns[1].user_text, 'hello');
    assert.ok(!JSON.stringify(turns).includes('private'));
    assert.ok(!JSON.stringify(turns).includes('internal'));
    assert.deepEqual(read(path, 'conversation'), turns);
});
test('partial_utf8_records_wait_for_newline_and_rewrites_reset_projection', t => {
    const path = fixture(t), read = createTranscriptReader();
    writeFileSync(path, row(0, 'USER_INPUT', '你好'));
    const response = Buffer.from(row(1, 'PLANNER_RESPONSE', '世界'));
    appendFileSync(path, response.subarray(0, response.length - 3));
    assert.equal(read(path, 'session')[0].agent_text, '');
    appendFileSync(path, response.subarray(response.length - 3));
    assert.equal(read(path, 'session')[0].agent_text, '世界');
    writeFileSync(path, row(0, 'USER_INPUT', 'replacement'));
    assert.equal(read(path, 'session')[0].user_text, 'replacement');
    assert.equal(read(path, 'session')[0].agent_text, '');
});
test('duplicate_step_records_update_instead_of_duplicate_output', t => {
    const path = fixture(t), read = createTranscriptReader();
    writeFileSync(path, row(0, 'USER_INPUT', 'hello') + row(1, 'PLANNER_RESPONSE', 'a', { status: 'RUNNING' }));
    assert.equal(read(path, 'session')[0].agent_text, 'a');
    appendFileSync(path, row(1, 'PLANNER_RESPONSE', 'answer'));
    assert.equal(read(path, 'session')[0].agent_text, 'answer');
    assert.equal(read(path, 'session')[0].status, 'unknown');
});

test('unchanged_transcript_poll_uses_metadata_without_rereading_content', async t => {
    const fs = (await import('node:fs')).default;
    const { syncBuiltinESMExports } = await import('node:module');
    const path = fixture(t), read = createTranscriptReader();
    writeFileSync(path, row(0, 'USER_INPUT', 'hello'));
    const original = fs.readSync;
    let bytesRead = 0;
    fs.readSync = (...args) => { const count = original(...args); bytesRead += count; return count; };
    syncBuiltinESMExports();
    try {
        const expected = read(path, 'native');
        const initialBytes = bytesRead;
        assert.ok(initialBytes > 0);
        for (let i = 0; i < 20; i++) assert.deepEqual(read(path, 'native'), expected);
        assert.equal(bytesRead, initialBytes);
        appendFileSync(path, row(1, 'PLANNER_RESPONSE', 'answer'));
        assert.equal(read(path, 'native')[0].agent_text, 'answer');
        assert.ok(bytesRead > initialBytes);
    } finally { fs.readSync = original; syncBuiltinESMExports(); }
});

test('native_usage_keeps_only_observed_counters_and_deduplicates_steps', t => {
    const path = fixture(t);
    writeFileSync(path, row(0, 'USER_INPUT', 'hello')
        + row(1, 'PLANNER_RESPONSE', '', { input_tokens: 8357, cache_read_tokens: 0, output_tokens: 273 })
        + row(1, 'PLANNER_RESPONSE', 'answer', { input_tokens: 8357, cache_read_tokens: 0, output_tokens: 280 })
        + row(2, 'GENERIC', 'tool', { input_tokens: 99 })
        + row(3, 'PLANNER_RESPONSE', 'done', { input_tokens: -1, output_tokens: 1.5 }));
    assert.deepEqual(createTranscriptReader()(path, 'native')[0].token_usage, [
        { id: 'native:step:1', input: 8357, cached: 0, output: 280 },
    ]);
});

test('native_user_request_envelope_excludes_metadata_and_preserves_plain_text', t => {
    const path = fixture(t);
    const wrapped = '<USER_REQUEST>\n hey\n </USER_REQUEST>\n <ADDITIONAL_METADATA>\nThe current local time is: 2026-11-11T10:36:29+08:00.\n</ADDITIONAL_METADATA>\n<USER_SETTINGS_CHANGE>\nModel Selection changed.\n</USER_SETTINGS_CHANGE>';
    writeFileSync(path, row(0, 'USER_INPUT', wrapped)
        + row(1, 'USER_INPUT', '<USER_REQUEST>\r\nfirst\r\n  second\r\n</USER_REQUEST>')
        + row(2, 'USER_INPUT', 'Explain <USER_REQUEST> literally')
        + row(3, 'USER_INPUT', '<USER_REQUEST>unfinished'));
    assert.deepEqual(createTranscriptReader()(path, 'native').map(turn => turn.user_text),
        ['hey', 'first\r\n  second', 'Explain <USER_REQUEST> literally', '<USER_REQUEST>unfinished']);
});

test('wrapped_native_input_acknowledges_plain_im_prompt_and_emits_only_user_body', async t => {
    const path = fixture(t), events = [];
    writeFileSync(path, '');
    const identity = { conversationId: 'native', workspacePaths: [tmpdir()], transcriptPath: path };
    const session = new AntigravitySession({ event: event => events.push(event), delivery: {
        send: async ({ text }) => {
            writeFileSync(path, row(0, 'USER_INPUT', `<USER_REQUEST>\n${text}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>time</ADDITIONAL_METADATA>`));
            session.hook({ ...identity, hook_event_name: 'PreInvocation' });
        },
    } });
    t.after(() => session.close());
    session.hook({ ...identity, hook_event_name: 'SessionStart' });
    assert.deepEqual(await session.request('prompt', { request_id: 'im-prompt', text: 'hey' }), { turn_id: 'native:step:0' });
    assert.equal(events.find(event => event.ItemCompleted?.item.kind === 'userMessage').ItemCompleted.item.text, 'hey');
    assert.equal(session.history().turns[0].user_text, 'hey');
});

test('bridge_messages_share_native_transcript_ids_across_replays_and_identical_turns', async t => {
    const { transcriptConversation } = await import('../../taskix-manager/conversation.mjs');
    const path = fixture(t), events = [];
    writeFileSync(path, '');
    const identity = { conversationId: 'native', workspacePaths: [tmpdir()], transcriptPath: path };
    const session = new AntigravitySession({ event: event => events.push(event) });
    t.after(() => session.close());
    session.hook({ ...identity, hook_event_name: 'SessionStart' });
    for (const start of [0, 3]) {
        appendFileSync(path, row(start, 'USER_INPUT', 'same prompt'));
        session.hook({ ...identity, hook_event_name: 'PreInvocation' });
        appendFileSync(path, row(start + 1, 'PLANNER_RESPONSE', 'progress'));
        session.hook({ ...identity, hook_event_name: 'PostInvocation' });
        appendFileSync(path, row(start + 2, 'PLANNER_RESPONSE', 'partial'));
        session.hook({ ...identity, hook_event_name: 'PostInvocation' });
        appendFileSync(path, row(start + 2, 'PLANNER_RESPONSE', 'done'));
        session.hook({ ...identity, hook_event_name: 'Stop', fullyIdle: true });
        session.hook({ ...identity, hook_event_name: 'Stop', fullyIdle: true });
    }
    const actual = new Map(events.flatMap(event => {
        const item = event.ItemCompleted?.item;
        return item && ['userMessage', 'agentMessage'].includes(item.kind)
            ? [[item.id, { id: item.id, role: item.kind === 'userMessage' ? 'user' : 'assistant', text: item.text }]] : [];
    }));
    const expected = await transcriptConversation(path, { sessionId: 'native', recentTurns: 2 });
    assert.deepEqual([...actual.values()], expected.messages);
    assert.equal(session.history().turns[0].agent_text, 'progress\ndone');
});
