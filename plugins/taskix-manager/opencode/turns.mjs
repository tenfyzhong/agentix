// Canonical OpenCode 2 tagged-message contract. Regenerate host copies with
// scripts/generate-opencode-turns.mjs after editing this file.
export function promptText(prompt = {}) {
    return [
        prompt.text || '',
        ...(prompt.files || []).map(
            (file) => `[Attached: ${file.name || file.description || 'file'}]`,
        ),
    ]
        .filter(Boolean)
        .join('\n');
}

/** Bounded ID-only state: the native transcript owns all prompt/file payloads. */
export class NativeTurns {
    constructor() {
        this.aliases = new Map();
    }
    user(id) {
        this.current = id;
        this.aliases.set(id, id);
        this.prune();
        return id;
    }
    assistant(id) {
        if (this.current) this.aliases.set(id, this.current);
        this.prune();
        return this.root(id);
    }
    root(id) {
        return this.aliases.get(id) ?? id;
    }
    prune() {
        for (const id of this.aliases.keys()) {
            if (this.aliases.size <= 4096) break;
            if (id !== this.current) this.aliases.delete(id);
        }
    }
}

/** Compaction/synthetic messages are separate types; they never become users. */
export function normalizeMessages(messages) {
    const turns = [],
        messageTurns = new Map(),
        native = new NativeTurns();
    let active = [];
    for (const message of messages) {
        if (message.type === 'user') {
            native.user(message.id);
            const turn = {
                id: message.id,
                status: 'inProgress',
                user_text: promptText(message),
                agent_text: '',
                tools: [],
                items: [],
            };
            turns.push(turn);
            active.push(turn);
            messageTurns.set(message.id, turn.id);
        } else if (message.type === 'assistant') {
            const turn = turns.at(-1);
            if (!turn) continue;
            native.assistant(message.id);
            messageTurns.set(message.id, turn.id);
            for (const part of message.content || []) {
                if (part.type === 'text') turn.agent_text += part.text;
                if (part.type === 'tool') {
                    const status = part.state.status;
                    const label = part.state.metadata?.title || part.name;
                    turn.tools.push({ kind: part.name, label, status });
                    turn.items.push({
                        id: part.id,
                        kind: 'tool',
                        text: label,
                        status,
                    });
                }
            }
            if (message.error) turn.status = 'failed';
        } else if (message.type === 'idle') {
            const status = {
                succeeded: 'completed',
                failed: 'failed',
                interrupted: 'interrupted',
            }[message.outcome];
            for (const turn of active) turn.status = status;
            active = [];
            native.current = undefined;
        }
    }
    for (const turn of turns)
        turn.items.push({
            id: turn.id + ':assistant',
            kind: 'agentMessage',
            text: turn.agent_text,
            status: turn.status,
        });
    return { turns, messageTurns };
}
