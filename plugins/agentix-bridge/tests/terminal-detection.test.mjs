import test from 'node:test';
import assert from 'node:assert/strict';
import { detectTerminal } from '../claude/terminal.mjs';

const env = { TMUX: '/tmp/native.sock,1,0', TMUX_PANE: '%1' };
function runner(roots) {
    return async (command) => {
        if (command === 'ps') return '100 1\n200 100\n300 200\n';
        if (!roots[command]) throw new Error('unavailable');
        return `${roots[command]}|omp|0|1|0|0`;
    };
}
test('tmux compatibility alias and rmux verifying the same pane are one target', async () => {
    const terminal = await detectTerminal({ env, pid: 300, run: runner({ rmux: 100, tmux: 100 }) });
    assert.equal(terminal.kind, 'rmux');
    assert.equal(terminal.pane, '%1');
});
test('different verified pane roots remain ambiguous', async () => {
    await assert.rejects(detectTerminal({ env, pid: 300, run: runner({ rmux: 100, tmux: 200 }) }), /uniquely/);
});
test('one valid driver is sufficient and absent ownership is rejected', async () => {
    assert.equal((await detectTerminal({ env, pid: 300, run: runner({ tmux: 100 }) })).kind, 'tmux');
    await assert.rejects(detectTerminal({ env, pid: 999, run: runner({ rmux: 100, tmux: 100 }) }), /uniquely/);
});
