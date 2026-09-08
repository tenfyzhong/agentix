import { Mailbox } from './mailbox.mjs';
let input = '';
for await (const chunk of process.stdin) {
    input += chunk;
    if (Buffer.byteLength(input) > 8 * 1024 * 1024) throw new Error('Claude hook exceeds maximum size');
}
try { new Mailbox().publish(JSON.parse(input)); }
catch (error) { console.error(`Agentix Claude hook: ${error.message}`); }
