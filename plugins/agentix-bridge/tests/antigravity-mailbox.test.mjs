import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AntigravityMailbox, normalizeHook } from '../antigravity/mailbox.mjs';
const native = { conversationId: 'native', workspacePaths: ['/work'], cwd: '/work', transcriptPath: '/sessions/native/transcript.jsonl', hookEventName: 'PreInvocation' };
test('native_hook_normalization_preserves_original_fields_and_validates_identity', () => {
    const event = normalizeHook(native);
    assert.equal(event.hook_event_name, 'PreInvocation');
    assert.equal(event.session_id, 'native');
    assert.equal(event.transcript_path, native.transcriptPath);
    assert.equal(event.conversationId, native.conversationId);
    assert.throws(() => normalizeHook({ ...native, transcriptPath: 'relative' }), /absolute/);
    assert.throws(() => normalizeHook({ ...native, conversationId: '' }), /identity/);
});
test('mailboxes_isolate_hosts_and_switch_native_identity', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agy-mailbox-')); t.after(() => rmSync(root, { recursive: true, force: true }));
    const one = new AntigravityMailbox(root, 'one'), two = new AntigravityMailbox(root, 'two');
    one.publish(native);
    assert.equal(one.identity().conversationId, 'native');
    assert.equal(two.identity(), null);
    one.publish({ ...native, conversationId: 'new-native' });
    assert.equal(one.identity().previous_session_id, 'native');
    one.publish({ ...native, conversationId: 'new-native', hookEventName: 'PostInvocation' });
    assert.equal(one.identity().previous_session_id, 'native');
    const events = []; await one.consume(event => events.push(event));
    assert.equal(events.length, 3);
    await one.consume(() => assert.fail('consumed events must not replay'));
});

test('hook_entrypoint_publishes_native_payload_without_polluting_stdout', { skip: process.platform === 'win32' }, async t => {
    const { execFile } = await import('node:child_process');
    const { hostKey } = await import('../claude/mailbox.mjs');
    const root = mkdtempSync(join(tmpdir(), 'agy-hook-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const mailbox = new AntigravityMailbox(root, hostKey(process.pid));
    const payload = { hookEventName: 'SessionStart', conversationId: 'native', workspacePaths: [root], transcriptPath: join(root, 'transcript.jsonl') };
    const result = await new Promise((resolve, reject) => {
        const child = execFile(process.execPath, [new URL('../antigravity/hook.mjs', import.meta.url).pathname],
            { env: { ...process.env, AGENTIX_ANTIGRAVITY_DATA_DIR: root } },
            (error, stdout, stderr) => error ? reject(error) : resolve({ stdout, stderr }));
        child.stdin.end(JSON.stringify(payload));
    });
    assert.equal(result.stdout, '{}\n');
    assert.equal(result.stderr, '');
    assert.equal(mailbox.identity().conversationId, 'native');
});
