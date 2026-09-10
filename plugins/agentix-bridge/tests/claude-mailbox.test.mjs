import test from 'node:test';
import assert from 'node:assert/strict';
import fs, { mkdtempSync, rmSync, mkdirSync, symlinkSync, realpathSync } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { EventEmitter } from 'node:events';
import { setImmediate as nextImmediate } from 'node:timers/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Mailbox } from '../claude/mailbox.mjs';

test('Claude hook mailbox isolates host processes and preserves bootstrap before MCP startup', async t => {
    const root = mkdtempSync(join(tmpdir(), 'agentix-claude-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const first = new Mailbox(root, 'host-one'), second = new Mailbox(root, 'host-two');
    const start = { hook_event_name: 'SessionStart', session_id: 'one', cwd: '/tmp', transcript_path: '/tmp/one.jsonl' };
    first.publish(start);
    first.publish({ ...start, hook_event_name: 'UserPromptSubmit', prompt: 'hello' });
    assert.equal(second.identity(), null);
    const consumer = new Mailbox(root, 'host-one');
    assert.deepEqual(consumer.identity(), start);
    const events = [];
    await consumer.consume(async event => events.push(event));
    assert.equal(events.length, 2);
    await consumer.consume(async () => assert.fail('event replayed'));
});

test('Claude mailbox keeps a failed event and later events until they are handled', async t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-mailbox-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const box = new Mailbox(root, 'host');
    const start = { hook_event_name: 'SessionStart', session_id: 's', cwd: root, transcript_path: join(root, 's.jsonl') };
    box.publish(start);
    box.publish({ ...start, hook_event_name: 'Stop' });
    await assert.rejects(box.consume(async () => { throw new Error('disk full'); }), /disk full/);
    const events = [];
    await box.consume(async event => events.push(event.hook_event_name));
    assert.deepEqual(events, ['SessionStart', 'Stop']);
    await box.consume(async () => assert.fail('event replayed'));
});

test('Claude mailbox watcher coalesces hook notifications and cancels pending wakes on shutdown', async t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-watch-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const box = new Mailbox(root, 'host');
    let notify;
    const watcher = new EventEmitter();
    watcher.close = t.mock.fn();
    t.mock.method(fs, 'watch', (_path, callback) => { notify = callback; return watcher; });
    syncBuiltinESMExports();
    t.after(() => { t.mock.restoreAll(); syncBuiltinESMExports(); });
    let calls = 0;
    const stop = box.watch(() => { calls++; });
    t.after(stop);
    // Native notifications can be delayed or lost; test the callback contract
    // independently of the OS. The server tests cover real mailbox consumption.
    notify('rename', 'partial.tmp');
    await nextImmediate();
    assert.equal(calls, 0);
    notify('rename', 'identity.json');
    notify('change', '001.event.json');
    assert.equal(calls, 0, 'notifications are deferred and coalesced');
    await nextImmediate();
    assert.equal(calls, 1);
    notify('rename', null);
    await nextImmediate();
    assert.equal(calls, 2, 'missing filenames still wake the consumer');
    notify('rename', '002.event.json');
    stop();
    notify('rename', '003.event.json');
    await nextImmediate();
    assert.equal(calls, 2, 'shutdown cancels queued and subsequent notifications');
    assert.equal(watcher.close.mock.callCount(), 1);
});

test('Claude mailbox watches the canonical directory behind aliases', t => {
    const root = mkdtempSync(join(tmpdir(), 'ax-watch-alias-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const actual = join(root, 'actual');
    mkdirSync(actual);
    const alias = join(root, 'alias');
    symlinkSync(actual, alias, process.platform === 'win32' ? 'junction' : 'dir');
    const box = new Mailbox(alias, 'host');
    assert.equal(box.path, realpathSync.native(box.path), 'reads, writes and watches must share the canonical path');
    let watched;
    t.mock.method(fs, 'watch', path => {
        watched = path;
        return { on() {}, close() {} };
    });
    syncBuiltinESMExports();
    t.after(() => { t.mock.restoreAll(); syncBuiltinESMExports(); });
    const stop = box.watch(() => {});
    stop();
    assert.equal(watched, realpathSync(box.path));
});
