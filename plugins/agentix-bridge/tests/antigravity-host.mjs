// Deterministic native-hook fixture for the Rust bridge contract tests.
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { AntigravityMailbox } from '../antigravity/mailbox.mjs';
import { createRuntime } from '../antigravity/runtime.mjs';

const [endpoint, root] = process.argv.slice(2);
const mailbox = new AntigravityMailbox(root, 'fixture-host');
let conversationId = 'native-antigravity', draft = process.argv.includes('--empty-draft') ? null : 'fixture draft', text = '';
const identity = () => ({ conversationId, workspacePaths: [root], transcriptPath: join(root, `${conversationId}.jsonl`) });
const publish = (hookEventName, extra = {}) => mailbox.publish({ ...identity(), hookEventName, ...extra });
writeFileSync(identity().transcriptPath, '');
publish('SessionStart');
let next = false;
const runtime = createRuntime({ root, mailbox, endpoint, clientId: 'fixture-host',
    nativeNew: async () => { next = true; },
    delivery: {
        draft: async expected => { if (expected === draft) draft = null; return draft; },
        send: async request => {
            if (next) { conversationId = 'replacement-antigravity'; next = false; text = ''; publish('SessionStart'); }
            const rows = text ? text.trim().split('\n').length : 0;
            text += JSON.stringify({ step_index: rows, type: 'USER_INPUT', source: 'USER_EXPLICIT', content: request.text }) + '\n';
            writeFileSync(identity().transcriptPath, text);
            publish('PreInvocation');
            text += JSON.stringify({ step_index: rows + 1, type: 'PLANNER_RESPONSE', source: 'MODEL', content: 'Antigravity reply' }) + '\n';
            writeFileSync(identity().transcriptPath, text);
            publish('Stop', { fullyIdle: true, terminationReason: 'NO_TOOL_CALL' });
            await runtime.poll();
        },
    },
});
await runtime.poll();
console.log('ready');
process.once('SIGTERM', async () => { await runtime.close(); process.exit(0); });
