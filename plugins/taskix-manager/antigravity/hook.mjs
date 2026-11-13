import { createAntigravityHooks } from './runtime.mjs';
import { reportHookError } from '../hooks/error-log.mjs';

let event;
try {
    const chunks = []; let bytes = 0;
    for await (const chunk of process.stdin) {
        bytes += chunk.length;
        if (bytes > 8 * 1024 * 1024) throw new Error('Antigravity hook input exceeds 8 MiB');
        chunks.push(chunk);
    }
    event = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    const configuredEvent = process.argv[2];
    if (configuredEvent !== undefined && event.hookEventName !== undefined && event.hookEventName !== configuredEvent) {
        throw new Error('Conflicting Antigravity hook event');
    }
    // Desktop App payloads omit the event name; CLI payloads include it.
    event.hookEventName = configuredEvent ?? event.hookEventName;
    process.stdout.write(JSON.stringify(await createAntigravityHooks().handle(event)) + '\n');
} catch (error) {
    process.exitCode = 1;
    await reportHookError(error, event && { hook_event_name: event.hookEventName, session_id: event.conversationId });
}
