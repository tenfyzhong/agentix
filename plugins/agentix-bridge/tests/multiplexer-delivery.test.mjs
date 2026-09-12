import test from 'node:test';
import assert from 'node:assert/strict';
import { TerminalDelivery, deliveryMode } from '../claude/delivery.mjs';

const env = { TMUX: '/tmp/arbitrary-socket,123,0', TMUX_PANE: '%7' };
function fixture(mode, available = ['tmux']) {
    const calls = [];
    const run = async (command, args, input) => {
        calls.push({ command, args, input });
        if (command === 'ps') return args.includes('-p') ? '200 200' : '200 100\n100 1\n';
        if (!available.includes(command)) throw new Error('wrong server');
        if (args.includes('display-message')) return '100|claude|2|1|0|0';
        if (args.includes('capture-pane')) return '──────────\n❯ \n──────────\n';
        return '';
    };
    return { calls, delivery: new TerminalDelivery({ mode, env, pid: 200, run, delay: async () => {} }) };
}
test('delivery modes are explicit and default to auto', () => {
    assert.equal(deliveryMode(), 'auto');
    for (const mode of ['auto', 'multiplexer', 'channel']) assert.equal(deliveryMode(mode), mode);
    for (const mode of ['rmux', 'tmux', '', 'unknown']) assert.throws(() => deliveryMode(mode));
});
for (const kind of ['rmux', 'tmux']) {
    test(kind + ' auto probes original socket without mutating another driver', async () => {
        const { delivery, calls } = fixture('auto', [kind]);
        await delivery.send({ text: 'hello' });
        assert.ok(calls.some(c => c.command === kind && c.args.includes('paste-buffer')));
        assert.ok(calls.filter(c => c.command !== 'ps').every(c => c.args[0] === '-S' && c.args[1] === env.TMUX.split(',')[0]));
        assert.ok(calls.filter(c => c.command !== kind && c.command !== 'ps').every(c => c.args.includes('display-message')));
    });
}
test('auto rejects ambiguous and unverified servers before input', async () => {
    for (const available of [[], ['rmux', 'tmux']]) {
        const { delivery, calls } = fixture('auto', available);
        await assert.rejects(delivery.send({ text: 'hello' }), e => e.notSent === true);
        assert.ok(!calls.some(c => c.args.includes('load-buffer') || c.args.includes('send-keys')));
    }
});
test('multiplexer uses registration config and refreshes it on reconnect', async () => {
    const { delivery, calls } = fixture('multiplexer', ['rmux', 'tmux']);
    await assert.rejects(delivery.send({ text: 'hello' }), /configuration/i);
    delivery.configure({ multiplexer: { kind: 'tmux' } });
    await delivery.send({ text: 'hello' });
    assert.ok(!calls.some(c => c.command === 'rmux'));
    calls.length = 0;
    delivery.configure({ multiplexer: { kind: 'rmux' } });
    await delivery.send({ text: 'again' });
    assert.ok(!calls.some(c => c.command === 'tmux'));
    assert.throws(() => delivery.configure({}), /configuration/i);
    await assert.rejects(delivery.send({ text: 'hello' }), /configuration/i);
});

test('auto preserves the original terminal context when the environment changes', async () => {
    const { delivery, calls } = fixture('auto');
    const original = env.TMUX;
    try {
        env.TMUX = '/tmp/replacement,1,0';
        await delivery.send({ text: 'hello' });
        assert.ok(calls.filter(c => c.command !== 'ps').every(c => c.args[1] === original.split(',')[0]));
    } finally {
        env.TMUX = original;
    }
});
