import { ChannelDelivery, TerminalDelivery, deliveryMode } from './delivery.mjs';
import { Server, StdioServerTransport, CallToolRequestSchema, ListToolsRequestSchema } from './vendor/sdk.mjs';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { BridgeTransport } from '../transport.mjs';
import { readTranscript } from './history.mjs';
import { ClaudeSession } from './session.mjs';
import { Mailbox, dataRoot } from './mailbox.mjs';
import { ClaudeStateStore } from './store.mjs';

const mailbox = new Mailbox();
const mode = deliveryMode(process.env.AGENTIX_CLAUDE_DELIVERY);
const channelMode = mode === 'channel';
let session, polling = false;
const mcp = new Server({ name: 'agentix-bridge', version: '0.1.0' }, {
    capabilities: { ...(channelMode ? { experimental: { 'claude/channel': {} } } : {}), tools: {} },
    instructions: channelMode ? 'Messages from this channel are Agentix user requests for this original session. Before acting on each message, call agentix_acknowledge with its request_id. Send your response through agentix_reply with the same request_id. Do not invent IDs. A reply does not end the turn; finish your normal turn after replying.' : undefined,
});
const delivery = channelMode
    ? new ChannelDelivery(value => mcp.notification(value)) : new TerminalDelivery({ mode });
const transport = new BridgeTransport({ registered: result => delivery.configure?.(result),
    registrationError: error => console.error('Agentix Claude registration: ' + error.message), snapshot: () => session.info(),
    handle: (method, params) => session.request(method, params), dispatch: work => work() });
const string = { type: 'string', minLength: 1 };
mcp.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: channelMode ? [
    { name: 'agentix_acknowledge', description: 'Acknowledge an Agentix channel request before doing its work.', inputSchema: { type: 'object', properties: { request_id: string }, required: ['request_id'], additionalProperties: false } },
    { name: 'agentix_reply', description: 'Send the response to the acknowledged Agentix request.', inputSchema: { type: 'object', properties: { request_id: string, text: string }, required: ['request_id', 'text'], additionalProperties: false } },
] : [] }));
mcp.setRequestHandler(CallToolRequestSchema, async request => {
    try {
        if (!channelMode) throw new Error('Channel tools are disabled for terminal delivery');
        if (!session) throw new Error('Claude SessionStart hook has not registered');
        const args = request.params.arguments ?? {};
        if (request.params.name === 'agentix_acknowledge') session.acknowledge(args.request_id);
        else if (request.params.name === 'agentix_reply') session.reply(args.request_id, args.text);
        else throw new Error('Unknown tool');
        return { content: [{ type: 'text', text: 'Recorded by Agentix bridge.' }] };
    } catch (error) { return { isError: true, content: [{ type: 'text', text: error.message }] }; }
});
async function poll() {
    if (polling) return;
    polling = true;
    try {
        const identity = mailbox.identity();
        if (!identity) return;
        if (session?.identity.session_id !== identity.session_id) {
            session?.close(); await transport.close();
            const directory = join(dataRoot(), 'sessions'); mkdirSync(directory, { recursive: true, mode: 0o700 });
            const path = join(directory, createHash('sha256').update(identity.transcript_path).digest('hex') + '.json');
            const store = new ClaudeStateStore(path);
            session = new ClaudeSession({ saved: store.load(() => readTranscript(identity.transcript_path, identity.session_id)), append: value => store.append(value),
                delivery, event: value => transport.event(value), sequence: () => transport.sequence });
            session.hook(identity);
            transport.open({ agent: 'claude', instance: session.instance, pid: process.ppid,
                session_id: identity.session_id, cwd: identity.cwd, session_file: identity.transcript_path });
        }
        await mailbox.consume(async event => {
            session.hook(event);
            if (event.session_id === session.identity.session_id && event.hook_event_name === 'SessionEnd') {
                transport.event({ SessionExited: { session_id: event.session_id } });
                await transport.close(true);
            }
        });
    } catch (error) { console.error(`Agentix Claude bridge: ${error.message}`); }
    finally { polling = false; }
}
await mcp.connect(new StdioServerTransport());
const stopWatching = mailbox.watch(poll);
const timer = setInterval(poll, 1000);
await poll();
let closing = false;
async function close() {
    if (closing) return;
    closing = true; stopWatching(); clearInterval(timer);
    try { session?.close(); } catch (error) { console.error(`Agentix Claude shutdown: ${error.message}`); }
    await transport.close(true); process.exit(0);
}
process.stdin.on('end', close);
process.once('SIGTERM', close);
process.once('SIGINT', close);
