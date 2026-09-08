import test from 'node:test';
import assert from 'node:assert/strict';
import { ClaudeSession } from '../claude/session.mjs';
import { validate } from '../protocol/validate.mjs';

const identity = { session_id: 'claude-one', transcript_path: '/tmp/projects/claude-one.jsonl', cwd: '/tmp', hook_event_name: 'SessionStart' };
function setup() {
    const sent = [], events = [];
    const session = new ClaudeSession({ notify: async value => sent.push(value), event: value => events.push(value) });
    session.hook(identity);
    return { session, sent, events };
}
test('Claude uses bridge snapshot schema and waits for explicit message acknowledgement', async () => {
    const { session, sent, events } = setup();
    assert.equal(validate('Snapshot', session.snapshot()), true);
    const pending = session.request('prompt', { request_id: 'one', text: 'hello' });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(sent[0].method, 'notifications/claude/channel');
    assert.equal(events.length, 0);
    session.acknowledge('one');
    const result = await pending;
    assert.ok(result.turn_id);
    session.reply('one', 'world');
    assert.equal(session.snapshot().turns[0].agent_text, 'world');
    assert.equal(session.snapshot().turns[0].status, 'inProgress');
    session.hook({ ...identity, hook_event_name: 'Stop' });
    assert.equal(session.snapshot().turns[0].status, 'completed');
    assert.equal(events.at(-1).TurnCompleted.turn_id, result.turn_id);
});
test('Claude deduplicates delivery and rejects changed request IDs and unsupported controls', async () => {
    const { session, sent } = setup();
    const first = session.request('prompt', { request_id: 'same', text: 'hello' });
    session.acknowledge('same');
    const receipt = await first;
    assert.deepEqual(await session.request('prompt', { request_id: 'same', text: 'hello' }), receipt);
    assert.equal(sent.length, 1);
    await assert.rejects(session.request('prompt', { request_id: 'same', text: 'changed' }), /different/);
    await assert.rejects(session.request('stop', {}), /Unsupported/);
    assert.ok(!session.snapshot().capabilities.includes('stop'));
});
test('Claude ignores foreign-session hooks and does not complete on a reply tool call', async () => {
    const { session } = setup();
    session.hook({ ...identity, hook_event_name: 'UserPromptSubmit', prompt: 'terminal input' });
    session.hook({ ...identity, session_id: 'other', hook_event_name: 'Stop' });
    assert.equal(session.snapshot().session.status, 'active');
    session.hook({ ...identity, hook_event_name: 'Stop', last_assistant_message: 'terminal answer' });
    assert.equal(session.snapshot().turns[0].agent_text, 'terminal answer');
});

test('Claude reports uncertain delivery and never automatically resends it', async () => {
    const sent = [];
    const session = new ClaudeSession({ notify: async value => sent.push(value), ackTimeout: 5 });
    session.hook(identity);
    await assert.rejects(session.request('prompt', { request_id: 'lost', text: 'once' }), error => error.bridgeCode === 'delivery_uncertain');
    await assert.rejects(session.request('prompt', { request_id: 'lost', text: 'once' }), error => error.bridgeCode === 'delivery_uncertain');
    assert.equal(sent.length, 1);
    assert.match((await session.request('command', { name: 'status' })).body, /lost.*uncertain/);
    session.acknowledge('lost');
    assert.ok((await session.request('prompt', { request_id: 'lost', text: 'once' })).turn_id);
});

test('an uncertain Claude delivery can be explicitly cleared without replaying it', async () => {
    const session = new ClaudeSession({ notify: async () => {}, ackTimeout: 5 }); session.hook(identity);
    await assert.rejects(session.request('prompt', { request_id: 'lost', text: 'old' }));
    await session.request('queue_clear');
    assert.throws(() => session.acknowledge('lost'), /cleared/);
    const next = session.request('prompt', { request_id: 'new', text: 'new' });
    session.acknowledge('new');
    assert.ok((await next).turn_id);
});

test('Claude failed receipt persistence does not submit or retain delivery ownership', async () => {
    let fail = true;
    const sent = [];
    const session = new ClaudeSession({ notify: async v => sent.push(v),
        append: () => { if (fail) throw new Error('disk full'); } });
    session.hook(identity);
    await assert.rejects(session.request('prompt', { request_id: 'retry', text: 'hello' }), /disk full/);
    assert.equal(sent.length, 0);
    assert.equal(session.queueState().uncertain, null);
    fail = false;
    const pending = session.request('prompt', { request_id: 'retry', text: 'hello' });
    session.acknowledge('retry');
    await pending;
});

test('Claude receipt timeout still rejects when persisting uncertainty fails', async () => {
    let writes = 0;
    const session = new ClaudeSession({ notify: async () => {}, ackTimeout: 5,
        append: () => { if (++writes > 1) throw new Error('disk full'); } });
    session.hook(identity);
    await assert.rejects(session.request('prompt', { request_id: 'timeout', text: 'hello' }), /disk full/);
    assert.equal(session.queueState().uncertain.id, 'timeout');
});

test('Claude retries completion after a failed state write without losing the active turn', () => {
    let fail = false;
    const session = new ClaudeSession({ append: () => { if (fail) throw new Error('disk full'); } });
    session.hook(identity);
    session.hook({ ...identity, hook_event_name: 'UserPromptSubmit', prompt: 'hello' });
    const stop = { ...identity, hook_event_name: 'Stop', last_assistant_message: 'answer' };
    fail = true;
    assert.throws(() => session.hook(stop), /disk full/);
    assert.equal(session.info().session.status, 'active');
    fail = false;
    session.hook(stop);
    assert.equal(session.history().turns.at(-1).status, 'completed');
});

test('Claude cannot clear uncertain ownership when persistence fails', async () => {
    let fail = false;
    const session = new ClaudeSession({ notify: async () => {}, ackTimeout: 5,
        append: () => { if (fail) throw new Error('disk full'); } });
    session.hook(identity);
    await assert.rejects(session.request('prompt', { request_id: 'lost', text: 'hello' }));
    fail = true;
    await assert.rejects(session.request('queue_clear'), /disk full/);
    assert.equal(session.queueState().uncertain.id, 'lost');
    await assert.rejects(session.request('prompt', { request_id: 'new', text: 'hello' }), /busy/);
});
