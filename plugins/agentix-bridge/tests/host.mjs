// Reusable subprocess fixture for Rust transport tests. No model/provider calls.
import { registerBridge } from '../runtime.mjs';
import { hostFixture } from './support.mjs';
import { randomUUID } from 'node:crypto';
import { existsSync, writeFileSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
const host = hostFixture();
const kind = process.argv[3] ?? 'pi';
if (process.argv[4]) {
    host.ctx.cwd = process.argv[4];
    host.ctx.sessionManager.getSessionFile = () => `${process.argv[4]}/session.jsonl`;
}
let abortGeneration = 0;
host.api.sendUserMessage = async text => {
    const generation = abortGeneration;
    host.ctx.busy = true;
    if (text === 'wait-for-test-ack') {
        writeFileSync(`${host.ctx.cwd}/${kind}.prompt-waiting`, 'waiting');
        while (!existsSync(`${host.ctx.cwd}/${kind}.prompt-release`)) await delay(5);
        if (generation !== abortGeneration) return;
    }
    setImmediate(async () => {
        await host.emit('agent_start');
        host.entries.push({ type: 'message', id: randomUUID(), message: { role: 'user', content: text } });
        await host.emit('message_start', { message: { role: 'user', content: text } });
        if (text === 'hold') return;
        await host.emit('message_update', { assistantMessageEvent: { type: 'text_delta', delta: 'bridge answer' } });
        host.entries.push({ type: 'message', id: randomUUID(), message: { role: 'assistant', content: [{ type: 'text', text: 'bridge answer' }] } });
        host.ctx.busy = false;
        await host.emit(kind === 'pi' ? 'agent_settled' : 'agent_end');
    });
};
host.ctx.abort = () => {
    abortGeneration++;
    host.ctx.busy = false;
    setImmediate(() => host.emit(kind === 'pi' ? 'agent_settled' : 'agent_end'));
};
const bridge = registerBridge(host.api, kind, { endpoint: process.argv[2] });
await host.emit('session_start');
console.log('ready');
process.on('SIGTERM', async () => { await bridge.close(); process.exit(0); });

// Stand in for the native interactive CLI event loop, including while disconnected.
setInterval(() => {}, 1000);
