import { isAbsolute, join } from 'node:path';
import { homedir } from 'node:os';
import { Mailbox, hostKey, atomicJson } from '../claude/mailbox.mjs';
import { createTranscriptReader } from './history.mjs';

export const dataRoot = () => process.env.AGENTIX_ANTIGRAVITY_DATA_DIR ?? join(homedir(), '.local/share/agentix/antigravity');
export function normalizeHook(value) {
    if (typeof value.conversationId !== 'string' || !value.conversationId || !Array.isArray(value.workspacePaths) ||
        !value.workspacePaths.length || typeof value.hookEventName !== 'string' || !value.hookEventName) {
        throw new Error('Invalid Antigravity hook identity');
    }
    const cwd = value.cwd ?? value.workspacePaths[0];
    if (typeof cwd !== 'string' || !isAbsolute(cwd) || typeof value.transcriptPath !== 'string' ||
        !isAbsolute(value.transcriptPath) || value.workspacePaths.some(path => typeof path !== 'string' || !isAbsolute(path))) {
        throw new Error('Antigravity hook paths must be absolute');
    }
    return { ...value, hook_event_name: value.hookEventName, session_id: value.conversationId,
        transcript_path: value.transcriptPath, cwd };
}

/** Reuse the private filesystem mailbox transport, not Claude's hook semantics. */
export class AntigravityMailbox extends Mailbox {
    readTranscript = createTranscriptReader();
    constructor(root = dataRoot(), key = hostKey()) { super(root, key); }
    publish(value) {
        const event = normalizeHook(value), previous = this.identity();
        // Bind at publication time, before later invocations append another user input.
        event.agentix_turn_id = this.readTranscript(event.transcriptPath, event.conversationId).at(-1)?.id ?? null;
        const switched = previous && previous.conversationId !== event.conversationId;
        const prior = switched ? previous.conversationId : previous?.previous_session_id;
        const identity = { ...event, ...(prior ? { previous_session_id: prior } : {}) };
        // Any native hook can recover identity when the MCP child starts after SessionStart.
        atomicJson(join(this.path, 'identity.json'), identity);
        super.publish(event);
        // The shared transport treats SessionStart specially; retain agy's switch identity.
        if (event.hook_event_name === 'SessionStart') atomicJson(join(this.path, 'identity.json'), identity);
    }
}
