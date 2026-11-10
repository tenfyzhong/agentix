import { Server, StdioServerTransport, ListToolsRequestSchema } from '../claude/vendor/sdk.mjs';
import { hostKey } from '../claude/mailbox.mjs';
import { AntigravityMailbox } from './mailbox.mjs';
import { createRuntime } from './runtime.mjs';
import { AntigravityDelivery } from './delivery.mjs';
import { AntigravityControls } from './controls.mjs';
import { AntigravityInteractions } from './interactions.mjs';
import { NativeSessionControl } from '../new-session.mjs';

const clientId = hostKey();
const mailbox = new AntigravityMailbox();
const report = error => console.error(`Agentix Antigravity bridge: ${error.message}`);
const delivery = new AntigravityDelivery();
const sessionControl = new NativeSessionControl('antigravity', {
    resolve: () => delivery.resolve(), ready: terminal => delivery.check(terminal),
    stop: () => delivery.stop(),
});
const runtime = createRuntime({ mailbox, clientId, delivery, controls: new AntigravityControls(delivery),
    nativeNew: busy => sessionControl.newSession(busy),
    interactions: new AntigravityInteractions(delivery), onError: report });
const mcp = new Server({ name: 'agentix-bridge', version: '0.2.0' }, { capabilities: { tools: {} } });
mcp.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: [] }));
await mcp.connect(new StdioServerTransport());
const poll = () => runtime.poll().catch(report);
const stopWatching = mailbox.watch(poll);
const timer = setInterval(poll, 1000);
await poll();
let closing = false;
async function close() {
    if (closing) return;
    closing = true;
    stopWatching(); clearInterval(timer);
    try { await runtime.close(); } catch (error) { report(error); }
    await mcp.close();
}
process.stdin.once('end', close);
process.once('SIGTERM', close);
process.once('SIGINT', close);
