import { randomUUID } from 'node:crypto';
import { failure } from './protocol/errors.mjs';
export const textOf = content => typeof content === 'string' ? content : (content ?? []).filter(p => p.type === 'text').map(p => p.text).join('\n');

/** Native conversation projection and stable turn identities, without transport or delivery. */
export class SessionState {
    #index;
    constructor(ctx, saved = {}) {
        this.ctx = ctx;
        this.updatedAt = Math.floor(Date.now() / 1000);
        this.turn = structuredClone(saved.turn);
        this.restored = Boolean(saved.turn);
        this.pendingText = '';
        this.interrupted = false;
        this.historyIds = new Map(saved.history_ids ?? []);
    }
    touch() { this.#index = undefined; this.updatedAt = Math.floor(Date.now() / 1000); }
    abandon() {
        if (this.turn?.status === 'inProgress') { this.turn.status = 'unknown'; this.turn.error = 'Delivery outcome remains uncertain'; }
    }
    state() { return structuredClone({ turn: this.turn, history_ids: [...this.historyIds] }); }
    summary(name = null) {
        return { id: this.ctx.sessionManager.getSessionId(), name, preview: this.pendingText || null,
            cwd: this.ctx.cwd, updatedAt: this.updatedAt, status: this.ctx.isIdle() ? 'idle' : 'active', terminal: null };
    }
    finish() {
        if (!this.turn || this.turn.status !== 'inProgress') return false;
        this.associate();
        this.turn.status = this.interrupted ? 'interrupted' : this.turn.error ? 'failed' : 'completed';
        return true;
    }
    history(cursor, requestedLimit = 20) {
        this.associate();
        const leaf = this.ctx.sessionManager.getLeafId?.();
        if (!this.#index || leaf == null || this.#index.leaf !== leaf) {
            const groups = [];
            for (const entry of this.ctx.sessionManager.getBranch?.() ?? this.ctx.sessionManager.getEntries()) {
                if (entry.type !== 'message') continue;
                if (entry.message?.role === 'user') groups.push({ id: this.historyIds.get(entry.id) ?? entry.id, messages: [entry.message] });
                else if (groups.length) groups.at(-1).messages.push(entry.message);
            }
            this.#index = { leaf, groups, ids: new Set(groups.map(group => group.id)) };
        }
        const extra = this.turn && !this.#index.ids.has(this.turn.id) ? { id: this.turn.id, messages: [] } : null;
        const groups = this.#index.groups;
        const length = groups.length + (extra ? 1 : 0);
        const limit = Math.max(1, Math.min(20, Number(requestedLimit)));
        const end = cursor == null ? length : Math.min(length, Number(cursor));
        if (!Number.isInteger(end) || end < 0 || !Number.isInteger(limit)) throw failure('invalid_request', 'Invalid history cursor or limit');
        const startIndex = Math.max(0, end - limit);
        const bounded = value => typeof value === 'string' && value.length > 100000 ? '[Earlier text omitted from this history page]\n' + value.slice(-100000) : value;
        const page = groups.slice(startIndex, end);
        if (extra && end > groups.length) page.push(extra);
        const turns = page.map(group => {
            let value;
            if (this.turn?.id === group.id && (!this.restored || !group.messages.length)) value = { ...this.turn };
            else {
                value = { id: group.id, status: 'completed', user_text: textOf(group.messages[0]?.content), agent_text: '', tools: [], items: [] };
                for (const message of group.messages.slice(1)) {
                    if (message?.role === 'assistant') {
                        value.agent_text = bounded(value.agent_text + textOf(message.content));
                        if (message.stopReason === 'aborted') value.status = 'interrupted';
                        if (message.stopReason === 'error') value.status = 'failed';
                    } else if (message?.role === 'toolResult') value.tools.push({ kind: message.toolName ?? 'tool', label: message.toolName ?? 'tool', status: message.isError ? 'failed' : 'completed' });
                }
            }
            if (this.restored && this.turn?.id === group.id) {
                if (group.messages.some(message => message?.role === 'assistant')) {
                    if (!this.ctx.isIdle() && value.status === 'completed') value.status = 'inProgress';
                    this.turn = { ...this.turn, ...value }; this.restored = false;
                } else value = { ...this.turn };
            }
            return { id: value.id, status: value.status, user_text: bounded(value.user_text), agent_text: bounded(value.agent_text), tools: value.tools, items: value.items.slice(-20) };
        });
        return { turns, older_cursor: startIndex > 0 ? String(startIndex) : null, newer_cursor: end < length ? String(Math.min(length, end + limit)) : null };
    }
    start() {
        if (this.turn?.status === 'inProgress') return false;
        this.interrupted = false;
        this.restored = false;
        this.touch();
        this.turn = { previous_user_id: (this.ctx.sessionManager.getBranch?.() ?? this.ctx.sessionManager.getEntries()).findLast(e => e.type === 'message' && e.message?.role === 'user')?.id, id: randomUUID(), status: 'inProgress', user_text: this.pendingText || null, agent_text: '', tools: [], items: [] };
        return true;
    }
    associate() {
        if (!this.turn || (this.turn.user_entry_id && this.historyIds.get(this.turn.user_entry_id) === this.turn.id)) return;
        const entries = this.ctx.sessionManager.getBranch?.() ?? this.ctx.sessionManager.getEntries();
        let user;
        if (this.turn.user_entry_id) user = entries.find(entry => entry.id === this.turn.user_entry_id);
        else {
            const previous = entries.findIndex(entry => entry.id === this.turn.previous_user_id);
            user = entries.slice(previous + 1).find(entry => entry.type === 'message' && entry.message?.role === 'user'
                && (this.turn.user_timestamp != null ? entry.message.timestamp === this.turn.user_timestamp : textOf(entry.message.content) === this.turn.user_text));
        }
        if (!user?.id) return;
        this.turn.user_entry_id = user.id;
        this.historyIds.set(user.id, this.turn.id);
        this.#index = undefined;
    }
}
