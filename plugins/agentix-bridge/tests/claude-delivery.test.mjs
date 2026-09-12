import test from 'node:test';
import assert from 'node:assert/strict';
import { TerminalDelivery } from '../claude/delivery.mjs';
import { ClaudeSession } from '../claude/session.mjs';
const identity = { session_id: 'one', cwd: '/tmp', transcript_path: '/tmp/one.jsonl', hook_event_name: 'SessionStart' };
for (const kind of ['rmux', 'tmux']) {
    function fixture(overrides = {}) {
        const calls = [];
        let cleared = false;
        const run = async (command, args, input) => {
            calls.push({ command, args, input });
            if (args.includes('C-c') && !overrides.clearFails) cleared = true;
            if (command === 'ps') return args.includes('-p') ? (overrides.foreground ?? '200 200') : '200 100\n100 1\n';
            if (args.includes('display-message')) return (!cleared && overrides.state) || '100|claude|2|1|0|0';
            if (args.includes('capture-pane')) return (!cleared && overrides.screen) || '──────────\n❯ \n──────────\n  -- INSERT --\n';
            return '';
        };
        const delivery = new TerminalDelivery({ mode: 'multiplexer', env: { TMUX: '/tmp/rmux-501/default,1,0', TMUX_PANE: '%91' }, pid: 200, run, delay: async () => {} });
        delivery.configure({ multiplexer: { kind } });
        return { calls, delivery };
    }
    test(kind + ' pastes literal multiline text into the original pane before Enter', async () => {
        const { calls, delivery } = fixture();
        await delivery.send({ request_id: 'one', text: 'hello\n$(not a shell command)' });
        assert.equal(calls.find(c => c.args.includes('load-buffer')).input, 'hello\n$(not a shell command)');
        const paste = calls.find(c => c.args.includes('paste-buffer'));
        assert.ok(paste.args.includes('-p')); assert.ok(paste.args.includes('%91'));
        assert.deepEqual(calls.find(c => c.args.includes('send-keys')).args.slice(-1), ['Enter']);
        assert.ok(calls.filter(c => c.command === kind).every(c => c.args[0] === '-S'));
    });
    for (const overrides of [ { foreground: '200 300' }, { state: '100|fish|2|1|0|0' }, { state: '999|claude|2|1|0|0' }, { state: '100|claude|2|1|1|0' }, { screen: 'Allow this tool?\n❯ Yes\n' } ]) {
        test(`${kind} rejects an unsafe pane: ${JSON.stringify(overrides)}`, async () => {
            const { delivery, calls } = fixture(overrides);
            await assert.rejects(delivery.send({ request_id: 'one', text: 'hello' }));
            assert.ok(!calls.some(c => c.args.includes('paste-buffer') || c.args.includes('send-keys')));
        });
    }
    test(kind + ' rejects missing pane context and terminal control input', async () => {
        await assert.rejects(new TerminalDelivery({ mode: 'multiplexer', env: {} }).send({ text: 'hello' }));
        const { delivery, calls } = fixture();
        await assert.rejects(delivery.send({ text: '\x1b[2J' }));
        await assert.rejects(delivery.send({ text: '/exit' }));
        assert.equal(calls.length, 0);
    });
    test(kind + ' delivery is acknowledged by the matching prompt hook and completed by Stop', async () => {
        const sent = [];
        const session = new ClaudeSession({ delivery: { kind: 'multiplexer', usesPromptHook: true, send: async r => sent.push(r) }, ackTimeout: 100 });
        session.hook(identity);
        const pending = session.request('prompt', { request_id: 'one', text: 'hello' });
        await new Promise(resolve => setImmediate(resolve));
        assert.equal(sent.length, 1);
        assert.equal(session.turns.length, 0);
        session.hook({ ...identity, hook_event_name: 'UserPromptSubmit', prompt: 'hello' });
        const result = await pending;
        assert.equal(session.turns.length, 1);
        session.hook({ ...identity, hook_event_name: 'Stop', last_assistant_message: 'answer' });
        assert.equal(session.turns[0].agent_text, 'answer');
        assert.deepEqual(await session.request('prompt', { request_id: 'one', text: 'hello' }), result);
        assert.equal(sent.length, 1);
    });
    test(kind + ' refuses unmatched hooks as delivery acknowledgements and preserves uncertainty', async () => {
        const session = new ClaudeSession({ delivery: { kind: 'multiplexer', usesPromptHook: true, send: async () => {} }, ackTimeout: 10 });
        session.hook(identity);
        const pending = session.request('prompt', { request_id: 'one', text: 'remote' });
        session.hook({ ...identity, hook_event_name: 'UserPromptSubmit', prompt: 'local' });
        await assert.rejects(pending, e => e.bridgeCode === 'delivery_uncertain');
        assert.equal(session.turns[0].user_text, 'local');
    });
    test('preflight failures leave no uncertain receipt and allow a later retry', async () => {
        let attempts = 0;
        const session = new ClaudeSession({ delivery: { kind: 'multiplexer', usesPromptHook: true, send: async () => {
            attempts++; throw Object.assign(new Error('Draft in input'), { notSent: true });
        } } }); session.hook(identity);
        await assert.rejects(session.request('prompt', { request_id: 'one', text: 'hello' }), /Draft/);
        assert.equal(session.queueState().uncertain, null);
        await assert.rejects(session.request('prompt', { request_id: 'one', text: 'hello' }));
        assert.equal(attempts, 2);
    });
    test('a partial paste failure is uncertain and never automatically resent', async () => {
        let attempts = 0;
        const session = new ClaudeSession({ delivery: { kind: 'multiplexer', usesPromptHook: true, send: async () => {
            attempts++; throw new Error('Enter failed');
        } } }); session.hook(identity);
        await assert.rejects(session.request('prompt', { request_id: 'one', text: 'hello' }), e => e.bridgeCode === 'delivery_uncertain');
        await assert.rejects(session.request('prompt', { request_id: 'one', text: 'hello' }));
        assert.equal(attempts, 1);
    });
    test('Claude status identifies its prompt delivery adapter', async () => {
        const session = new ClaudeSession({ delivery: { kind: 'multiplexer', usesPromptHook: true, send: async () => {} } }); session.hook(identity);
        assert.match((await session.request('command', { name: 'status' })).body, /Delivery: multiplexer/);
    });
    test('native ' + kind + ' clears a multiline draft and submits only the IM prompt', {
        skip: !(kind === 'tmux' ? process.env.AGENTIX_TEST_TMUX : process.env.AGENTIX_TEST_NATIVE_HOSTS) || process.platform === 'win32', timeout: 15000,
    }, async t => {
        const { execFileSync } = await import('node:child_process');
        const { mkdtempSync, readFileSync, rmSync, copyFileSync, mkdirSync, symlinkSync, realpathSync, existsSync } = await import('node:fs');
        const { tmpdir } = await import('node:os');
        const { join, dirname } = await import('node:path');
        const { waitFor } = await import('./support.mjs');
        const root = mkdtempSync(join(tmpdir(), 'ax-rmux-')), socket = join(root, 'rmux.sock'), output = join(root, 'input');
        t.after(() => { try { execFileSync(kind, ['-S', socket, 'kill-server']); } catch {} rmSync(root, { recursive: true, force: true }); });
        const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
        mkdirSync(join(root, 'bin'));
        copyFileSync(process.execPath, join(root, 'bin/claude'));
        const libraries = join(dirname(realpathSync(process.execPath)), '../lib');
        if (existsSync(libraries)) symlinkSync(libraries, join(root, 'lib'), 'dir');
        const command = 'exec ' + quote(join(root, 'bin/claude')) + ' ' + [new URL('./claude-rmux-host.mjs', import.meta.url).pathname, output, 'local draft\nsecond line'].map(quote).join(' ');
        const pane = execFileSync(kind, ['-S', socket, 'new-session', '-d', '-s', 'test', '-x', '100', '-y', '30', '-P', '-F', '#{pane_id}', command], { encoding: 'utf8' }).trim();
        await waitFor(() => execFileSync(kind, ['-S', socket, 'capture-pane', '-p', '-t', pane], { encoding: 'utf8' }).includes('❯'), 5000);
        const pid = Number(execFileSync(kind, ['-S', socket, 'display-message', '-p', '-t', pane, '#{pane_pid}'], { encoding: 'utf8' }).trim());
        const delivery = new TerminalDelivery({ mode: 'multiplexer', env: { TMUX: `${socket},1,0`, TMUX_PANE: pane }, pid });
        delivery.configure({ multiplexer: { kind } });
        const text = 'hello\n世界 $(literal) `also literal`';
        const state = await delivery.configured.inspect();
        assert.equal(state[1], 'claude', JSON.stringify(state));
        assert.equal(state[4], '0', JSON.stringify(state));
        assert.equal(state[5], '0', JSON.stringify(state));
        delivery.mode = 'auto';
        await delivery.send({ request_id: 'native', text });
        await waitFor(() => { try { return readFileSync(output, 'utf8') === text; } catch { return false; } });
        assert.equal(readFileSync(output, 'utf8'), text);
    });
    test('an aborted submission after paste never presses Enter', async () => {
        const { delivery, calls } = fixture();
        const controller = new AbortController();
        delivery.delay = async () => controller.abort();
        await assert.rejects(delivery.send({ text: 'hello', signal: controller.signal }));
        assert.ok(calls.some(c => c.args.includes('paste-buffer')));
        assert.ok(!calls.some(c => c.args.includes('send-keys')));
    });
    test('closing a session cancels outstanding delivery work', async () => {
        let signal;
        const session = new ClaudeSession({ delivery: { kind: 'multiplexer', usesPromptHook: true, send: async r => { signal = r.signal; } } }); session.hook(identity);
        const pending = session.request('prompt', { request_id: 'one', text: 'hello' });
        await new Promise(resolve => setImmediate(resolve));
        session.close();
        await assert.rejects(pending, e => e.bridgeCode === 'delivery_uncertain');
        assert.equal(signal.aborted, true);
    });

    for (const draft of [
        { state: '100|claude|4|1|0|0', screen: '──────────\n❯ local draft\n──────────\n' },
        { state: '100|claude|3|2|0|0', screen: '──────────\n❯ first line\n  second line\n──────────\n' },
    ]) {
        test('clears the entire terminal draft before pasting IM text: ' + draft.state, async () => {
            const { calls, delivery } = fixture(draft);
            await delivery.send({ text: 'remote only' });
            const clear = calls.findIndex(c => c.args.includes('C-c'));
            const paste = calls.findIndex(c => c.args.includes('paste-buffer'));
            assert.ok(clear >= 0 && clear < paste);
            assert.equal(calls.filter(c => c.args.includes('C-c')).length, 1);
            assert.equal(calls.find(c => c.args.includes('load-buffer')).input, 'remote only');
        });
    }
    test('does not paste or submit when draft clearing fails', async () => {
        const { calls, delivery } = fixture({ clearFails: true, screen: '──────────\n❯ draft\n──────────\n' });
        await assert.rejects(delivery.send({ text: 'remote' }));
        assert.ok(calls.some(c => c.args.includes('C-c')));
        assert.ok(!calls.some(c => c.args.includes('paste-buffer') || c.args.includes('Enter')));
    });
    test('an already empty input never receives Ctrl+C', async () => {
        const { calls, delivery } = fixture();
        await delivery.send({ text: 'remote' });
        assert.ok(!calls.some(c => c.args.includes('C-c')));
    });

}
