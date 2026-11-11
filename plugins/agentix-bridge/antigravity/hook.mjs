import { AntigravityMailbox } from './mailbox.mjs';

try {
    const chunks = [];
    let size = 0;
    for await (const chunk of process.stdin) {
        size += chunk.length;
        if (size > 8 * 1024 * 1024) throw new Error('Antigravity hook exceeds maximum size');
        chunks.push(chunk);
    }
    new AntigravityMailbox().publish(JSON.parse(Buffer.concat(chunks).toString('utf8')));
} catch (error) {
    console.error(`Agentix Antigravity hook: ${error.message}`);
}
// An unavailable bridge must not terminate the user's native invocation.
process.stdout.write('{}\n');
