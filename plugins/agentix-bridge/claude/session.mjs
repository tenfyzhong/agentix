import { randomUUID } from 'node:crypto';
import { ChannelDelivery } from './delivery.mjs';
import { failure } from '../protocol/errors.mjs';

/** Converts Claude lifecycle hooks and delivery receipts to the existing bridge contract. */
export class ClaudeSession {
    constructor(options = {}) {
        this.options = options;
        this.delivery = options.delivery ?? new ChannelDelivery(options.notify);
        this.instance = randomUUID();
        this.turns = structuredClone(options.saved?.turns ?? []);
        this.receipts = new Map(options.saved?.receipts ?? []);
        this.waiters = new Map();
        this.active = null;
        this.identity = null;
        this.updatedAt = Math.floor(Date.now() / 1000);
        for (const receipt of this.receipts.values()) {
            if (receipt.state === 'pending') receipt.state = 'uncertain';
        }
        for (const turn of this.turns) if (turn.status === 'inProgress') turn.status = 'unknown';
    }
    save(receiptIds = []) {
        this.updatedAt = Math.floor(Date.now() / 1000);
        this.options.append?.({
            turn: this.active ?? this.turns.at(-1),
            receipts: receiptIds.map(id => [id, this.receipts.get(id) ?? null]),
        });
        this.options.persist?.({ turns: this.turns, receipts: [...this.receipts] });
    }
    emit(event) { this.options.event?.(event); }
    info() {
        return { instance: this.instance, seq: this.options.sequence?.() ?? 0,
            session: { id: this.identity.session_id, cwd: this.identity.cwd, name: null,
                preview: this.turns.at(-1)?.user_text ?? null, updatedAt: this.updatedAt,
                status: this.active ? 'active' : 'idle', terminal: null },
            capabilities: ['prompt', 'history', 'status', 'queue_control'] };
    }
    snapshot() { return { ...this.info(), turns: this.history().turns, queue: this.queueState() }; }
    queueState() {
        const unresolved = [...this.receipts].find(([, r]) => ['pending', 'uncertain'].includes(r.state));
        return { items: [], paused: Boolean(unresolved), uncertain: unresolved ? { id: unresolved[0], text: unresolved[1].text } : null };
    }
    history(cursor, limit = 20) {
        const end = cursor == null ? this.turns.length : Number(cursor);
        if (!Number.isInteger(end) || end < 0 || end > this.turns.length || !Number.isInteger(limit) || limit < 1) throw failure('invalid_request', 'Invalid history cursor or limit');
        const size = Math.min(limit, 20), start = Math.max(0, end - size);
        return { turns: structuredClone(this.turns.slice(start, end)), older_cursor: start ? String(start) : null,
            newer_cursor: end < this.turns.length ? String(Math.min(this.turns.length, end + size)) : null };
    }
    begin(text, id = randomUUID(), receiptIds = []) {
        const turn = { id, status: 'inProgress', user_text: text, agent_text: '', tools: [], items: [] };
        const previous = this.active;
        this.turns.push(turn); this.active = turn;
        try { this.save(receiptIds); }
        catch (error) { this.turns.pop(); this.active = previous; throw error; }
        this.emit({ TurnStarted: { session_id: this.identity.session_id, turn_id: id } });
        this.emit({ ItemCompleted: { session_id: this.identity.session_id, turn_id: id,
            item: { id: `${id}:user`, kind: 'userMessage', text, status: 'completed' } } });
        return turn;
    }
    async request(method, params = {}) {
        if (!this.identity) throw failure('session_changed', 'Claude session has not registered');
        if (method === 'info') return this.info();
        if (method === 'snapshot') return this.snapshot();
        if (method === 'history') return this.history(params.cursor, params.limit);
        if (method === 'queue_state') return this.queueState();
        if (method === 'queue_resume') {
            if (this.queueState().uncertain) throw failure('delivery_uncertain', 'Inspect history and clear the uncertain delivery first');
            return { message: 'No pending deliveries' };
        }
        if (method === 'queue_clear') {
            if (this.active || this.waiters.size) throw failure('busy', 'Wait for the active turn or delivery acknowledgement');
            const cleared = [];
            for (const [id, receipt] of this.receipts) if (receipt.state === 'uncertain') { receipt.state = 'cleared'; cleared.push(id); }
            try { this.save(cleared); }
            catch (error) { for (const id of cleared) this.receipts.get(id).state = 'uncertain'; throw error; }
            return { message: 'Uncertain delivery cleared; no message was cancelled or resent' };
        }
        if (method === 'command' && params.name === 'status') return { body: `Session: ${this.identity.session_id}\nWorkspace: ${this.identity.cwd}\nState: ${this.active ? 'active' : 'idle'}\nDelivery: ${this.delivery.kind}\nUnresolved deliveries: ${[...this.receipts].filter(([, r]) => ['pending', 'uncertain'].includes(r.state)).map(([id, r]) => `${id}: ${r.state}`).join(', ') || 'none'}`, choices: [] };
        if (method !== 'prompt') throw failure('unsupported_method', `Unsupported Claude operation: ${method}`);
        const { request_id: id, text } = params;
        if (typeof id !== 'string' || !id || typeof text !== 'string' || !text.trim()) throw failure('invalid_request', 'request_id and nonempty text are required');
        const old = this.receipts.get(id);
        if (old) {
            if (old.text !== text) throw failure('invalid_request', 'request_id reused with different text');
            if (old.state === 'accepted') return { turn_id: old.turn_id };
            throw failure('delivery_uncertain', 'Claude delivery is pending or uncertain; do not resend');
        }
        if (this.active || [...this.receipts.values()].some(r => ['pending', 'uncertain'].includes(r.state))) throw failure('busy', 'Claude is busy or has an uncertain delivery');
        const receipt = { text, turn_id: randomUUID(), state: 'pending' };
        this.receipts.set(id, receipt);
        try { this.save([id]); }
        catch (error) { this.receipts.delete(id); throw error; }
        return new Promise((resolve, reject) => {
            const controller = new AbortController();
            const timer = setTimeout(() => {
                controller.abort();
                this.waiters.delete(id); receipt.state = 'uncertain';
                let message = 'Claude has not acknowledged this message';
                try { this.save([id]); } catch (error) { message += `; state persistence failed: ${error.message}`; }
                reject(failure('delivery_uncertain', message));
            }, this.options.ackTimeout ?? 7000);
            this.waiters.set(id, { resolve, reject, timer, controller });
            Promise.resolve().then(() => this.delivery.send({ request_id: id, text, signal: controller.signal })).catch(error => {
                if (!this.waiters.has(id)) return;
                clearTimeout(timer); this.waiters.delete(id);
                if (error.notSent) this.receipts.delete(id);
                else receipt.state = 'uncertain';
                try { this.save([id]); }
                catch (storageError) {
                    receipt.state = 'uncertain'; this.receipts.set(id, receipt);
                    reject(failure('delivery_uncertain', `State persistence failed: ${storageError.message}`));
                    return;
                }
                reject(error.notSent ? error : failure('delivery_uncertain', error.message));
            });
        });
    }
    acknowledge(id) {
        const receipt = this.receipts.get(id);
        if (!receipt) throw failure('invalid_request', 'Unknown delivery');
        if (receipt.state === 'cleared') throw failure('invalid_request', 'Delivery was explicitly cleared');
        if (receipt.state === 'accepted') return { turn_id: receipt.turn_id };
        if (this.active) throw failure('busy', 'Another turn is active');
        const previous = receipt.state;
        receipt.state = 'accepted';
        try { this.begin(receipt.text, receipt.turn_id, [id]); }
        catch (error) { receipt.state = previous; throw error; }
        const waiter = this.waiters.get(id);
        if (waiter) { clearTimeout(waiter.timer); this.waiters.delete(id); waiter.resolve({ turn_id: receipt.turn_id }); }
        return { turn_id: receipt.turn_id };
    }
    reply(id, text) {
        const receipt = this.receipts.get(id);
        if (!receipt || receipt.state !== 'accepted' || this.active?.id !== receipt.turn_id) throw failure('invalid_request', 'No matching acknowledged turn');
        if (typeof text !== 'string' || !text.trim()) throw failure('invalid_request', 'Reply text is required');
        this.active.agent_text = text.slice(-100000); this.save();
        this.emit({ ItemCompleted: { session_id: this.identity.session_id, turn_id: this.active.id,
            item: { id: `${this.active.id}:assistant`, kind: 'agentMessage', text: this.active.agent_text, status: 'completed' } } });
    }
    hook(value) {
        if (!this.identity) {
            if (value.hook_event_name !== 'SessionStart') return;
            this.identity = value;
            return;
        }
        if (value.session_id !== this.identity.session_id) return;
        this.identity.cwd = value.cwd ?? this.identity.cwd;
        if (value.hook_event_name === 'UserPromptSubmit' && !this.active) {
            const match = this.delivery.kind === 'rmux' && [...this.receipts].find(([, r]) =>
                ['pending', 'uncertain'].includes(r.state) && r.text === value.prompt);
            if (match) this.acknowledge(match[0]);
            else this.begin(value.prompt ?? '');
        }
        if (['Stop', 'StopFailure', 'SessionEnd'].includes(value.hook_event_name) && this.active) {
            const turn = this.active;
            const previous = { status: turn.status, agent_text: turn.agent_text };
            if (!turn.agent_text && typeof value.last_assistant_message === 'string') turn.agent_text = value.last_assistant_message.slice(-100000);
            turn.status = value.hook_event_name === 'Stop' ? 'completed' : value.hook_event_name === 'SessionEnd' ? 'interrupted' : 'failed';
            this.active = null;
            try { this.save(); }
            catch (error) { Object.assign(turn, previous); this.active = turn; throw error; }
            this.emit({ ItemCompleted: { session_id: value.session_id, turn_id: turn.id,
                item: { id: `${turn.id}:assistant`, kind: 'agentMessage', text: turn.agent_text, status: turn.status } } });
            this.emit({ TurnCompleted: { session_id: value.session_id, turn_id: turn.id, status: turn.status, error: value.error ?? null } });
        }
    }
    close() {
        for (const [id, waiter] of this.waiters) {
            waiter.controller.abort(); clearTimeout(waiter.timer); this.receipts.get(id).state = 'uncertain';
            waiter.reject(failure('delivery_uncertain', 'Claude bridge disconnected'));
        }
        const ids = [...this.waiters.keys()];
        this.waiters.clear(); this.save(ids);
    }
}
