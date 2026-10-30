import test from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { RexTerminalAdapter, rexCandidates, decodeRexVT } from '../claude/rex.mjs';

const env = { REX_SESSION: 'session:one', REX_BLOCK: 'block:one', PATH: '/bin', HOME: '/home/test' };
test('Rex executable lookup preserves PATH before macOS application locations', () => {
    const paths = rexCandidates(env, 'darwin');
    assert.equal(paths[0], join('/bin', 'rex'));
    assert.equal(paths[1], join('/Applications', 'Rex.app', 'Contents/Helpers/Rex Server.rex/Contents/MacOS/rex'));
    assert.equal(rexCandidates(env, 'linux').length, 1);
});
test('Rex VT snapshot decodes wide cells, erased padding and final cursor', () => {
    const result = decodeRexVT('old\r\n───\r\n❯ 中文\x1b[2X\r\n───\x1b[3;3H', 4, 10);
    assert.equal(result.x, 2); assert.equal(result.y, 2);
    assert.equal(result.text.split('\n')[2].trimEnd(), '❯ 中文');
    assert.throws(() => decodeRexVT('\x1b[999;2H', 4, 10));
    assert.throws(() => decodeRexVT('missing cursor', 4, 20));
});
test('Rex verifies original local process and pastes to its full ID without autostart', async () => {
    const calls = [];
    const run = async (command, args) => {
        calls.push({ command, args });
        if (command === 'ps') return args.includes('-A') ? '100 1\n200 100\n' : '200 200 claude\n';
        if (args.includes('whoami')) return JSON.stringify({ endpoint: 'unix:///tmp/rex.sock', host_id: 'local' });
        if (args.includes('inspect')) return JSON.stringify({ block: { host_id: 'local', creator_name: 'com.superlogical.terminal' } });
        if (args.includes('process')) return JSON.stringify({ child: { pid: 100 }, foreground: { pid: 200, name: 'claude' }, last_exit: null });
        if (args.includes('size')) return JSON.stringify({ rows: 4, columns: 20 });
        if (args.includes('format')) return JSON.stringify({ content: '───\r\n❯ \r\n───\r\n\x1b[2;3H' });
        return '{}';
    };
    const terminal = new RexTerminalAdapter({ env, pid: 200, run, command: '/bin/rex' });
    const state = await terminal.inspect();
    assert.deepEqual(state, ['100', 'claude', '2', '1', '0', '0']);
    await terminal.paste('first\nsecond');
    const call = calls.find(c => c.args.includes('block.event'));
    const payload = JSON.parse(call.args.at(-1));
    assert.equal(payload.block_id, env.REX_BLOCK);
    assert.deepEqual(payload.event, { type: 'paste', payload: { text: 'first\nsecond' } });
    assert.ok(calls.filter(c => c.command !== 'ps').every(c => c.args.includes('--autostart=false')));
});

import { TerminalDelivery } from '../claude/delivery.mjs';
function deliveryFixture(scenario = 'ready') {
    const calls = []; let pasted = false;
    const run = async (command, args) => {
        calls.push({ command, args });
        if (command === 'ps') return args.includes('-A') ? '100 1\n200 100\n' : scenario === 'background' ? '200 300\n' : '200 200\n';
        if (args.includes('whoami')) return JSON.stringify({ endpoint: scenario === 'remote' ? 'tcp://remote:1234' : 'unix:///tmp/rex.sock', host_id: 'local' });
        if (args.includes('inspect')) return JSON.stringify({ block: { host_id: 'local', creator_name: 'com.superlogical.terminal' } });
        if (args.includes('process')) return JSON.stringify({ child: { pid: scenario === 'foreign' ? 999 : 100 }, foreground: { pid: 200, name: 'claude' }, last_exit: scenario === 'dead' ? { exit_code: 0 } : null });
        if (args.includes('size')) return JSON.stringify({ rows: 4, columns: 30 });
        if (args.includes('format')) return JSON.stringify({ content: scenario === 'draft' ? '───\r\n❯ draft\r\n───\r\n\x1b[2;8H' : '───\r\n❯ \r\n───\r\n\x1b[2;3H' });
        if (args.includes('block.event')) { pasted = true; if (scenario === 'paste-error') throw new Error('connection lost'); }
        return '{}';
    };
    const delivery = new TerminalDelivery({ mode: 'multiplexer', env, pid: 200, command: '/bin/rex', run, delay: async () => {} });
    delivery.configure({ multiplexer: { kind: 'rex' } });
    return { delivery, calls, pasted: () => pasted };
}
test('configured Rex delivery uses native paste then Enter', async () => {
    const { delivery, calls } = deliveryFixture();
    await delivery.send({ text: 'first\n中文' });
    const writes = calls.filter(c => c.args.includes('block.event') || c.args.includes('send-key'));
    assert.equal(writes.length, 2);
    assert.equal(JSON.parse(writes[0].args.at(-1)).event.payload.text, 'first\n中文');
    assert.equal(writes[1].args.at(-1), 'enter');
});
for (const scenario of ['draft', 'remote', 'dead', 'foreign', 'background']) test(`Rex delivery refuses ${scenario} before writing`, async () => {
    const { delivery, calls } = deliveryFixture(scenario);
    await assert.rejects(delivery.send({ text: 'hello' }), error => error.notSent === true);
    assert.ok(!calls.some(c => c.args.includes('block.event') || c.args.includes('send-key') || c.args.includes('write')));
});
test('a Rex paste failure remains uncertain and never presses Enter', async () => {
    const { delivery, calls, pasted } = deliveryFixture('paste-error');
    await assert.rejects(delivery.send({ text: 'hello' }), error => error.notSent !== true);
    assert.ok(pasted()); assert.ok(!calls.some(c => c.args.includes('send-key')));
});
test('Rex draft capture does not include screen padding', async () => {
    const { delivery } = deliveryFixture('draft');
    assert.equal(await delivery.draft(), 'draft');
});
test('joined emoji occupies one wide grapheme and cannot overflow a narrow terminal', () => {
    assert.equal(decodeRexVT('👩‍💻abc\x1b[1;6H', 2, 6).text.split('\n')[0], '👩‍💻abc');
    assert.throws(() => decodeRexVT('中\x1b[1;1H', 2, 1));
});
