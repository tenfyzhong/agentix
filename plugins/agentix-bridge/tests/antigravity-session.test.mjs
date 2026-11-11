import test from 'node:test';
import assert from 'node:assert/strict';
import { AntigravitySession } from '../antigravity/session.mjs';
const identity = { conversationId: 'native', workspacePaths: ['/work'], transcriptPath: '/sessions/native/transcript.jsonl' };
const turn = (id = 'native:step:0', text = 'hello', answer = '') => ({ id, user_text: text, agent_text: answer, status: 'unknown', items: [], tools: [] });
function fixture(options = {}) {
    let transcript = [turn()];
    const events = [], saved = [];
    const session = new AntigravitySession({ readTranscript: () => structuredClone(transcript), event: e => events.push(e), persist: s => saved.push(structuredClone(s)), ...options });
    const hook = (name, extra = {}) => session.hook({ ...identity, hook_event_name: name, ...extra });
    return { session, events, saved, hook, transcript: value => { transcript = value; } };
}
test('terminal_input_reads_and_clears_matching_draft_only_when_idle', async () => {
    const clears = [];
    const f = fixture({ delivery: { draft: async expected => { clears.push(expected); return expected ? null : 'draft'; } } });
    f.hook('SessionStart');
    assert.deepEqual(await f.session.request('terminal_input'), { draft: 'draft' });
    assert.deepEqual(await f.session.request('terminal_input', { clear: 'draft' }), { draft: null });
    f.hook('PreInvocation');
    assert.deepEqual(await f.session.request('terminal_input'), { draft: null });
    await assert.rejects(f.session.request('terminal_input', { clear: 'draft' }), /busy/i);
    assert.deepEqual(clears, [undefined, 'draft']);
});
test('a_new_native_invocation_can_request_the_same_approval_again', async () => {
    const prompt = { kind: 'file_approval', title: 'Allow access?', detail: '/tmp/file', choices: ['Yes', 'No'], fingerprint: 'same' };
    const f = fixture({ interactions: { inspect: async () => prompt, respond: async () => null } });
    f.hook('SessionStart'); f.hook('PreInvocation', { invocationNum: 0, initialNumSteps: 1 });
    await f.session.pollInteraction();
    const first = f.events.find(event => event.InteractionRequested).InteractionRequested;
    await f.session.request('interaction', { rpc_id: first.rpc_id, response: { decision: 'Yes' } });
    await f.session.pollInteraction();
    assert.equal(f.events.filter(event => event.InteractionRequested).length, 1);
    f.hook('PreInvocation', { invocationNum: 1, initialNumSteps: 3 });
    await f.session.pollInteraction();
    const requests = f.events.filter(event => event.InteractionRequested);
    assert.equal(requests.length, 2);
    assert.notEqual(requests[1].InteractionRequested.rpc_id, first.rpc_id);
});
test('invocations_do_not_duplicate_turns_and_background_stop_does_not_complete', () => {
    const f = fixture();
    f.hook('SessionStart'); f.hook('PreInvocation'); f.hook('PreInvocation');
    assert.equal(f.events.filter(e => e.TurnStarted).length, 1);
    f.transcript([turn(undefined, 'hello', 'answer')]);
    f.hook('PostInvocation'); f.hook('Stop', { fullyIdle: false });
    assert.equal(f.events.filter(e => e.TurnCompleted).length, 0);
    f.hook('Stop', { fullyIdle: true, terminationReason: 'model_stop' });
    f.hook('Stop', { fullyIdle: true, terminationReason: 'model_stop' });
    assert.equal(f.events.filter(e => e.TurnCompleted).length, 1);
    assert.equal(f.session.history().turns[0].agent_text, 'answer');
    assert.equal(f.session.info().session.status, 'idle');
});
test('same_text_in_distinct_native_steps_starts_distinct_turns', () => {
    const f = fixture(); f.hook('SessionStart'); f.hook('PreInvocation');
    f.hook('Stop', { fullyIdle: true });
    f.transcript([turn(), turn('native:step:9')]); f.hook('PreInvocation');
    assert.equal(f.events.filter(e => e.TurnStarted).length, 2);
    f.session.hook({ ...identity, conversationId: 'foreign', hook_event_name: 'Stop', fullyIdle: true });
    assert.equal(f.session.info().session.status, 'active');
});
test('recovery_does_not_restart_completed_native_turn', () => {
    const f = fixture(); f.hook('SessionStart'); f.hook('PreInvocation'); f.hook('Stop', { fullyIdle: true });
    const resumed = fixture({ saved: f.saved.at(-1) });
    resumed.hook('SessionStart'); resumed.hook('PreInvocation');
    assert.equal(resumed.events.filter(e => e.TurnStarted).length, 0);
});
test('failed_state_write_does_not_publish_start', () => {
    const f = fixture({ persist: () => { throw new Error('disk full'); } });
    f.hook('SessionStart');
    assert.throws(() => f.hook('PreInvocation'), /disk full/);
    assert.equal(f.events.length, 0);
});

test('delivery_is_acknowledged_by_a_new_native_input_and_deduplicated', async () => {
    let calls = 0;
    const f = fixture({ delivery: { send: async () => { calls++; } } });
    f.hook('SessionStart');
    const pending = f.session.request('prompt', { request_id: 'delivery', text: 'hello' });
    await new Promise(resolve => setImmediate(resolve));
    f.transcript([turn(), turn('native:step:9')]); f.hook('PreInvocation');
    assert.deepEqual(await pending, { turn_id: 'native:step:9' });
    assert.deepEqual(await f.session.request('prompt', { request_id: 'delivery', text: 'hello' }), { turn_id: 'native:step:9' });
    assert.equal(calls, 1);
    await assert.rejects(f.session.request('prompt', { request_id: 'delivery', text: 'different' }), /reused/);
});
test('delivery_timeout_persists_uncertainty_and_never_resends', async () => {
    let calls = 0;
    const f = fixture({ ackTimeout: 10, delivery: { send: async () => { calls++; } } }); f.hook('SessionStart');
    await assert.rejects(f.session.request('prompt', { request_id: 'lost', text: 'hello' }), /acknowledged/);
    await assert.rejects(f.session.request('prompt', { request_id: 'lost', text: 'hello' }), /uncertain/);
    assert.equal(calls, 1);
    assert.equal(f.saved.at(-1).receipts[0][1].state, 'uncertain');
});

test('bridge_read_methods_and_close_preserve_uncertain_delivery', async () => {
    const f = fixture({ delivery: { send: async () => {} } }); f.hook('SessionStart');
    assert.equal((await f.session.request('info')).session.id, 'native');
    assert.deepEqual((await f.session.request('snapshot')).turns, [turn()]);
    assert.deepEqual((await f.session.request('history')).turns, [turn()]);
    assert.match((await f.session.request('command', { name: 'status' })).body, /native/);
    const pending = f.session.request('prompt', { request_id: 'closing', text: 'hello' });
    const rejected = assert.rejects(pending, /disconnected/);
    f.session.close(); await rejected;
    assert.equal(f.saved.at(-1).receipts[0][1].state, 'uncertain');
});

test('recovered_in_flight_turn_settles_on_native_stop_without_duplicate_start', () => {
    const first = fixture(); first.hook('SessionStart'); first.hook('PreInvocation');
    const recovered = fixture({ saved: first.saved.at(-1) });
    recovered.hook('SessionStart');
    assert.equal(recovered.session.history().turns[0].status, 'unknown');
    recovered.transcript([turn(undefined, 'hello', 'recovered answer')]);
    recovered.hook('Stop', { fullyIdle: true });
    assert.equal(recovered.session.history().turns[0].status, 'completed');
    assert.equal(recovered.session.history().turns[0].agent_text, 'recovered answer');
    assert.equal(recovered.events.filter(event => event.TurnStarted).length, 0);
    assert.equal(recovered.events.filter(event => event.TurnCompleted).length, 1);
});

test('uncertain_delivery_is_visible_in_snapshot_and_queue_state', async () => {
    const f = fixture({ saved: { receipts: [['lost', { text: 'hello', state: 'pending' }]] } });
    f.hook('SessionStart');
    assert.deepEqual(f.session.snapshot().queue.uncertain, { id: 'lost', text: 'hello' });
    assert.deepEqual(await f.session.request('queue_state'), f.session.snapshot().queue);
});

test('registration_imports_native_history_without_inventing_completion_or_events', () => {
    const f = fixture();
    f.transcript([turn('native:step:0', 'old', 'old answer'), turn('native:step:4', 'latest')]);
    f.hook('SessionStart');
    assert.deepEqual(f.session.history().turns.map(value => value.user_text), ['old', 'latest']);
    assert.ok(f.session.history().turns.every(value => value.status === 'unknown'));
    assert.deepEqual(f.events, []);
    f.hook('PreInvocation');
    assert.equal(f.events.filter(event => event.TurnStarted).length, 1);
    assert.equal(f.session.history().turns.length, 2);
    f.hook('Stop', { fullyIdle: true });
    assert.equal(f.session.history().turns[0].status, 'unknown');
    assert.equal(f.session.history().turns[1].status, 'completed');
});

test('changed_native_tool_items_are_published_once_and_persisted', () => {
    const f = fixture(); f.hook('SessionStart'); f.hook('PreInvocation');
    const item = { id: 'tool-1', kind: 'read_file', text: '{"path":"README.md"}', status: 'unknown' };
    f.transcript([{ ...turn(), items: [item], tools: [{ id: item.id, kind: item.kind, label: 'Read README', status: 'unknown' }] }]);
    f.hook('PostInvocation'); f.hook('PostInvocation');
    assert.deepEqual(f.events.filter(event => event.ItemCompleted?.item.id === 'tool-1'), [
        { ItemCompleted: { session_id: 'native', turn_id: 'native:step:0', item } },
    ]);
    assert.deepEqual(f.saved.at(-1).turns[0].items, [item]);
});

test('stop_requests_native_escape_and_waits_for_native_completion', async () => {
    let stops = 0;
    const f = fixture({ delivery: { send: async () => {}, stop: async () => { stops++; return null; } } });
    f.hook('SessionStart'); f.hook('PreInvocation');
    assert.ok(f.session.info().capabilities.includes('stop'));
    await f.session.request('stop');
    assert.equal(stops, 1);
    assert.equal(f.session.info().session.status, 'active');
    assert.equal(f.events.filter(event => event.TurnCompleted).length, 0);
    f.hook('Stop', { fullyIdle: true, terminationReason: 'interrupted' });
    assert.equal(f.session.history().turns[0].status, 'interrupted');
});

test('model_controls_are_serialized_against_prompt_delivery_and_active_turns', async () => {
    let finish, calls = 0;
    const f = fixture({ delivery: { send: async () => {} }, controls: { command: async () => {
        calls++; return new Promise(resolve => { finish = resolve; });
    } } });
    f.hook('SessionStart');
    assert.ok(f.session.info().capabilities.includes('model'));
    const pending = f.session.request('command', { name: 'model' });
    await assert.rejects(f.session.request('command', { name: 'reasoning' }), /busy/i);
    await assert.rejects(f.session.request('prompt', { request_id: 'during-control', text: 'hello' }), /busy/i);
    finish({ body: 'models', choices: [] });
    assert.equal((await pending).body, 'models');
    f.hook('PreInvocation');
    await assert.rejects(f.session.request('command', { name: 'model', value: 'model' }), /busy/i);
    assert.equal(calls, 1);
});

test('native_approval_is_published_once_and_resolved_without_replaying_input', async () => {
    const prompt = { kind: 'command_approval', title: 'Run this command?', detail: 'printf test', choices: ['Yes', 'No'], selected: 0, fingerprint: 'scope' };
    let answers = 0;
    const f = fixture({ interactions: { inspect: async () => prompt, respond: async (expected, response) => {
        assert.equal(expected.fingerprint, 'scope'); assert.equal(response.decision, 'Yes'); answers++;
    } } });
    f.hook('SessionStart'); f.hook('PreInvocation');
    await f.session.pollInteraction(); await f.session.pollInteraction();
    const events = f.events.filter(event => event.InteractionRequested);
    assert.equal(events.length, 1);
    const request = events[0].InteractionRequested;
    assert.equal(request.session_id, 'native');
    assert.deepEqual(request.available_decisions, ['Yes', 'No', 'cancel']);
    await f.session.request('interaction', { rpc_id: request.rpc_id, response: { decision: 'Yes' } });
    await assert.rejects(f.session.request('interaction', { rpc_id: request.rpc_id, response: { decision: 'Yes' } }), /pending/);
    await f.session.pollInteraction();
    assert.equal(answers, 1);
    assert.equal(f.events.filter(event => event.InteractionRequested).length, 1);
    assert.equal(f.events.filter(event => event.InteractionResolved).length, 1);
});

test('native_questions_project_shared_im_form_fields', async () => {
    const prompt = { kind: 'user_input', title: 'Pick features', detail: 'Pick features', questionId: 'question:2',
        choices: ['Logs', 'Metrics', 'Write-in...'], multiple: true, custom: true, checked: [], fingerprint: 'question' };
    const f = fixture({ interactions: { inspect: async () => prompt } });
    f.hook('SessionStart'); f.hook('PreInvocation'); await f.session.pollInteraction();
    const request = f.events.find(event => event.InteractionRequested).InteractionRequested;
    assert.deepEqual(request.available_decisions, []);
    assert.deepEqual(request.payload, { rejectable: true, questions: [{ id: 'question:2', header: 'Antigravity question',
        question: 'Pick features', multiple: true, custom: true, required: true, options: [{ label: 'Logs' }, { label: 'Metrics' }] }] });
});

test('uncertain_delivery_can_be_cleared_explicitly_without_resending', async () => {
    let sends = 0;
    const f = fixture({ saved: { receipts: [['lost', { text: 'hello', state: 'uncertain' }]] }, delivery: { send: async () => { sends++; } } });
    f.hook('SessionStart');
    await assert.rejects(f.session.request('queue_resume'), /uncertain/);
    await f.session.request('queue_clear');
    assert.equal((await f.session.request('queue_state')).uncertain, null);
    assert.equal(f.saved.at(-1).receipts[0][1].state, 'cleared');
    await f.session.request('queue_resume');
    await assert.rejects(f.session.request('prompt', { request_id: 'lost', text: 'hello' }), /uncertain|cleared/);
    assert.equal(sends, 0);
});
test('failed_uncertainty_clear_retains_the_recovery_gate', async () => {
    const f = fixture({ saved: { receipts: [['lost', { text: 'hello', state: 'uncertain' }]] }, persist: () => { throw new Error('disk full'); } });
    f.hook('SessionStart');
    await assert.rejects(f.session.request('queue_clear'), /disk full/);
    assert.deepEqual(f.session.queueState().uncertain, { id: 'lost', text: 'hello' });
});

for (const [reason, status] of [['TERMINAL_CUSTOM_HOOK', 'interrupted'], ['max_steps_exceeded', 'failed'],
    ['USER_CANCELED', 'interrupted'], ['MAX_INVOCATIONS', 'failed'], ['MAX_FORCED_INVOCATIONS', 'failed'], ['MAX_TOKEN_BUDGET_EXCEEDED', 'failed']]) {
    test(`native_stop_${reason.toLowerCase()}_does_not_claim_success`, () => {
        const f = fixture(); f.hook('SessionStart'); f.hook('PreInvocation');
        f.hook('Stop', { fullyIdle: true, terminationReason: reason });
        assert.equal(f.session.history().turns[0].status, status);
    });
}

test('usage_is_observed_only_on_active_live_invocations', () => {
    const observed = [];
    const f = fixture({ usage: (id, value) => observed.push([id, value]) });
    const native = { ...turn(), token_usage: [{ id: 'native:step:1', input: 10, output: 2 }] };
    f.transcript([native]); f.hook('SessionStart');
    assert.equal(observed.length, 0);
    f.session.history(); f.session.snapshot();
    assert.equal(observed.length, 0);
    f.hook('PreInvocation');
    assert.equal(observed.length, 0);
    f.hook('PostInvocation');
    assert.deepEqual(observed, [['native:step:1', native.token_usage[0]]]);
    f.hook('Stop', { fullyIdle: true });
    const count = observed.length;
    f.hook('Stop', { fullyIdle: true });
    assert.equal(observed.length, count);
});

test('persisted_wrapped_user_text_is_normalized_before_history_and_preview', () => {
    const f = fixture({ saved: { turns: [{ ...turn(undefined, '<USER_REQUEST>\nhey\n</USER_REQUEST>\n<ADDITIONAL_METADATA>time</ADDITIONAL_METADATA>'), status: 'completed' }] } });
    f.transcript([turn(undefined, 'hey')]);
    f.hook('SessionStart');
    assert.equal(f.session.history().turns[0].user_text, 'hey');
    assert.equal(f.session.info().session.preview, 'hey');
});
