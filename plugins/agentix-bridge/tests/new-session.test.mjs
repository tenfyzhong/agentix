import test from 'node:test';
import assert from 'node:assert/strict';
import { NativeSessionControl } from '../new-session.mjs';

for (const kind of ['omp', 'claude']) {
    test(`${kind}: native new validates pane, stops and submits only its fixed command`, async () => {
        const calls = []; let busy = true;
        const terminal = { pane: '%1', inspect: async () => ['1', kind, '0', '0', '0', '0'], foreground: async () => {},
            call: async args => calls.push(args) };
        const control = new NativeSessionControl(kind, { resolve: async () => terminal, ready: async () => assert.equal(busy, false),
            stop: async () => { calls.push(['stop']); busy = false; } });
        await control.newSession(() => busy);
        assert.deepEqual(calls, [['stop'], ['send-keys', '-t', '%1', '-l', kind === 'claude' ? '/clear' : '/new'], ['send-keys', '-t', '%1', 'Enter']]);
        terminal.inspect = async () => ['1', 'zsh', '0', '0', '0', '0'];
        await assert.rejects(control.newSession(() => false), /active/);
        assert.equal(calls.length, 3);
    });
}

for (const kind of ['omp', 'claude']) {
    for (const scenario of ['copy mode', 'dead pane', 'draft', 'foreground changed']) {
        test(`${kind}: native new rejects ${scenario} without submitting input`, async () => {
            const calls = [];
            const terminal = {
                pane: '%1',
                inspect: async () => ['1', kind, '0', '0', scenario === 'copy mode' ? '1' : '0', scenario === 'dead pane' ? '1' : '0'],
                foreground: async () => { if (scenario === 'foreground changed') throw new Error('foreground changed'); },
                call: async args => calls.push(args),
            };
            const control = new NativeSessionControl(kind, {
                resolve: async () => terminal,
                ready: async () => { if (scenario === 'draft') throw new Error('draft'); },
            });
            await assert.rejects(control.newSession(() => false));
            assert.deepEqual(calls, []);
        });
    }
    test(`${kind}: native new rechecks foreground before pressing Enter`, async () => {
        const calls = []; let foregroundChecks = 0;
        const terminal = {
            pane: '%1', inspect: async () => ['1', kind, '0', '0', '0', '0'],
            foreground: async () => { if (++foregroundChecks === 3) throw new Error('agent exited'); },
            call: async args => calls.push(args),
        };
        const control = new NativeSessionControl(kind, { resolve: async () => terminal, ready: async () => {} });
        await assert.rejects(control.newSession(() => false), /agent exited/);
        assert.deepEqual(calls, [['send-keys', '-t', '%1', '-l', kind === 'claude' ? '/clear' : '/new']]);
    });
}
