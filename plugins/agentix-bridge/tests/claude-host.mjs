// Simulates Claude's MCP client and lifecycle hooks; the production plugin handles Agentix RPCs.
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { join } from 'node:path';
import { Mailbox, hostKey } from '../claude/mailbox.mjs';
const [endpoint, root] = process.argv.slice(2);
const mailbox = new Mailbox(join(root, 'claude-data'), hostKey(process.pid));
const identity = { hook_event_name: 'SessionStart', session_id: 'native-claude', cwd: root, transcript_path: join(root, 'native-claude.jsonl') };
mailbox.publish(identity);
const child = spawn(process.execPath, [new URL('../claude/server.mjs', import.meta.url).pathname], {
    env: { ...process.env, AGENTIX_CLAUDE_DELIVERY: 'channel', AGENTIX_CLAUDE_DATA_DIR: join(root, 'claude-data'), AGENTIX_CONTROL_ENDPOINT: endpoint },
    stdio: ['pipe', 'pipe', 'inherit'],
});
const send = frame => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...frame }) + '\n');
let delivery;
createInterface({ input: child.stdout }).on('line', line => {
    const frame = JSON.parse(line);
    if (frame.id === 1) { send({ method: 'notifications/initialized' }); console.log('ready'); }
    if (frame.method === 'notifications/claude/channel') {
        delivery = frame.params.meta.request_id;
        send({ id: 2, method: 'tools/call', params: { name: 'agentix_acknowledge', arguments: { request_id: delivery } } });
    }
    if (frame.id === 2) send({ id: 3, method: 'tools/call', params: { name: 'agentix_reply', arguments: { request_id: delivery, text: 'Claude reply' } } });
    if (frame.id === 3) mailbox.publish({ ...identity, hook_event_name: 'Stop' });
});
send({ id: 1, method: 'initialize', params: { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'Claude fixture', version: '1' } } });
process.once('SIGTERM', () => child.kill());
child.on('exit', () => process.exit(0));
