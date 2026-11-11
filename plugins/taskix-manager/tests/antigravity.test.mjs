import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createAntigravityHooks, createAntigravityTool } from '../antigravity/runtime.mjs';

async function fixture(t, overrides = {}) {
    const root = await mkdtemp(join(tmpdir(), 'taskix-agy-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const transcriptPath = join(root, 'transcript.jsonl'), calls = [], captures = [];
    const write = rows => writeFile(transcriptPath, rows.map(row => JSON.stringify(row)).join('\n') + '\n');
    await write([{ step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'request' }]);
    const runner = async (args, options) => {
        calls.push({ args, options });
        if (args[0] === 'hook' && args[1] === 'record') captures.push(JSON.parse(await readFile(args[3], 'utf8')));
        return { schema_version: 1, ok: true, result: {} };
    };
    const hooks = createAntigravityHooks({ root, key: 'host', runner, routing: { env: {}, cacheDir: join(root, 'cache') }, ...overrides });
    const event = (hookEventName, extra = {}) => hooks.handle({ hookEventName, conversationId: 'native', workspacePaths: [root], transcriptPath, ...extra });
    return { root, hooks, event, calls, captures, write };
}

test('native_hooks_inject_context_and_capture_only_visible_native_messages', async t => {
    const f = await fixture(t);
    const start = await f.event('SessionStart');
    assert.match(start.injectSteps[0].ephemeralMessage, /Task session: native/);
    const first = await f.event('PreInvocation', { invocationNum: 0, initialNumSteps: 1 });
    assert.match(first.injectSteps.map(step => step.ephemeralMessage).join('\n'), /native:step:0/);
    assert.equal(f.captures[0].turn_id, 'native:step:0');
    assert.ok(Number.isSafeInteger(f.captures[0].messages[0].completed_at_ms));
    await f.write([
        { step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'request' },
        { step_index: 1, type: 'THINKING', source: 'MODEL', content: 'private' },
        { step_index: 2, type: 'PLANNER_RESPONSE', source: 'MODEL', content: 'reply' },
    ]);
    await f.event('Stop', { fullyIdle: false, terminationReason: 'NO_TOOL_CALL' });
    assert.equal(f.captures.at(-1).messages.at(-1).completed_at_ms, undefined);
    assert.deepEqual(await f.event('Stop', { fullyIdle: true, terminationReason: 'NO_TOOL_CALL' }), {});
    assert.ok(Number.isSafeInteger(f.captures.at(-1).messages.at(-1).completed_at_ms));
    assert.deepEqual(f.captures.at(-1).messages.map(message => message.text), ['request', 'reply']);
    assert.ok(f.calls.every(call => call.options.session === 'native'));
    assert.ok(f.calls.every(call => call.options.executor === 'agent:antigravity:native'));
    assert.ok(!f.calls.some(call => call.args[1] === 'session-end'));
});

test('model_invocations_route_once_per_explicit_native_user_step', async t => {
    const events = [];
    const f = await fixture(t, { hookRunner: async event => { events.push(event); return {}; } });
    await f.event('SessionStart');
    await f.event('PreInvocation', { invocationNum: 0 });
    await f.event('PreInvocation', { invocationNum: 1 });
    await f.event('PreToolUse');
    assert.equal(events.filter(event => event.hook_event_name === 'UserPromptSubmit').length, 1);
    await f.write([
        { step_index: 0, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'request' },
        { step_index: 4, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: 'request' },
    ]);
    await f.event('PreInvocation', { invocationNum: 0 });
    assert.deepEqual(events.filter(event => event.hook_event_name === 'UserPromptSubmit').map(event => event.turn_id), ['native:step:0', 'native:step:4']);
});

test('only_explicit_native_interruptions_release_task_leases', async t => {
    const f = await fixture(t);
    await f.event('Stop', { fullyIdle: false, terminationReason: 'TERMINAL_CANCELLED' });
    await f.event('PostToolUse', { error: 'tool failed' });
    assert.ok(!f.calls.some(call => call.args[1] === 'interrupt'));
    await f.event('Stop', { fullyIdle: true, terminationReason: 'USER_CANCELED' });
    assert.ok(f.calls.some(call => call.args[1] === 'interrupt'));
});
test('cancelled_issue_blocks_native_tools_without_auto_approving_permissions', async t => {
    const f = await fixture(t, { runner: async () => ({ result: { issue_cancellations: [{ id: 'issue_one', job_id: 'job_one' }] } }) });
    const result = await f.event('PreToolUse');
    assert.equal(result.decision, 'deny');
    assert.match(result.reason, /Issue work has been cancelled/);
    assert.equal(result.permissionOverrides, undefined);
});

test('native_conversation_switch_releases_only_previous_session_and_persisted_receipts_survive_restart', async t => {
    const events = [];
    const hookRunner = async event => { events.push(event); return {}; };
    const f = await fixture(t, { hookRunner });
    await f.event('SessionStart');
    await f.event('PreInvocation');
    const resumed = createAntigravityHooks({ root: f.root, key: 'host', hookRunner });
    const identity = await resumed.identity();
    await resumed.handle({ hookEventName: 'PreInvocation', conversationId: 'native', workspacePaths: [f.root], transcriptPath: identity.transcript_path });
    assert.equal(events.filter(event => event.hook_event_name === 'UserPromptSubmit').length, 1);
    await resumed.handle({ hookEventName: 'SessionStart', conversationId: 'next', workspacePaths: [f.root], transcriptPath: identity.transcript_path });
    assert.deepEqual(events.filter(event => event.hook_event_name === 'SessionEnd').map(event => event.session_id), ['native']);
    assert.equal((await resumed.identity()).routed_turn, undefined);
    assert.equal((await resumed.identity()).session_id, 'next');
});

test('failed_switch_initialization_never_leaves_the_tool_bound_to_the_old_identity', async t => {
    const f = await fixture(t);
    await f.event('SessionStart');
    const old = await f.hooks.identity();
    const broken = createAntigravityHooks({ root: f.root, key: 'host', hookRunner: async () => { throw new Error('storage unavailable'); } });
    await assert.rejects(broken.handle({ hookEventName: 'SessionStart', conversationId: 'next', workspacePaths: [f.root], transcriptPath: old.transcript_path }), /storage unavailable/);
    const current = await broken.identity();
    assert.equal(current.session_id, 'next');
    assert.equal(current.ready, false);
    await assert.rejects(createAntigravityTool()('request', { args: ['context'] }, current), /identity/i);
});

test('native_tool_binds_executor_session_and_current_lease_without_shell_parsing', async () => {
    const calls = [];
    const tool = createAntigravityTool({ runner: async (args, options) => {
        calls.push({ args, options });
        return { result: { task_id: 'task_one', lease: { token: 'lease_one' } } };
    } });
    const identity = { session_id: 'native', cwd: '/work', turn_id: 'native:step:0', ready: true };
    await tool('call', { args: ['task', 'done', 'task_one'] }, identity);
    assert.equal(calls.at(-1).options.session, 'native');
    assert.equal(calls.at(-1).options.executor, 'agent:antigravity:native');
    assert.equal(calls.at(-1).options.token, 'lease_one');
    assert.equal(calls.at(-1).options.idempotencyKey, 'antigravity:native:call');
    await tool('title', { args: ['task', 'update', 'task_one', '--title', '$(touch nope); title'] }, identity);
    assert.equal(calls.at(-1).args.at(-1), '$(touch nope); title');
    await assert.rejects(tool('invalid', { args: ['task', 'done', 'task_one'] }, null), /identity/i);
    await assert.rejects(tool('spoof', { args: ['context', '--session=other'] }, identity), /managed by the host/);
});

test('native_tool_discussion_selection_uses_the_original_user_step', async () => {
    const tool = createAntigravityTool({ routing: { env: {} }, runner: async args => {
        if (args[0] === 'conversation') return { result: { complete: true, revision: 1, turns: [{ turn_id: 'native:step:0' }] } };
        return { result: {} };
    } });
    const response = await tool('discussion', { args: ['conversation', 'classify', JSON.stringify({ title: 'Task', goal: 'Implement', prompt: 'request' })] },
        { session_id: 'native', cwd: '/work', turn_id: 'native:step:0', ready: true });
    assert.equal(response.details.status, 'selected');
    assert.deepEqual(response.details.turn_ids, ['native:step:0']);
});

test('ordinary_pre_tool_hook_delegates_to_native_permission_policy', async t => {
    const f = await fixture(t);
    assert.deepEqual(await f.event('PreToolUse', { toolCall: { name: 'run_command', args: { CommandLine: 'printf fixture' } } }),
        { decision: 'allow' });
});
