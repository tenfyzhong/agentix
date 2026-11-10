import { randomUUID } from 'node:crypto';
import { createTranscriptReader } from './history.mjs';
import { failure } from '../protocol/errors.mjs';

/** Native trajectory identity, separate from invocation and terminal process identity. */
export class AntigravitySession {
    constructor(options = {}) {
        this.options = options;
        this.instance = randomUUID();
        this.readTranscript = options.readTranscript ?? createTranscriptReader();
        this.turns = structuredClone(options.saved?.turns ?? []);
        this.receipts = new Map(options.saved?.receipts ?? []);
        this.waiters = new Map();
        this.active = this.turns.find(turn => turn.id === options.saved?.active_id || turn.status === 'inProgress') ?? null;
        this.identity = null;
        this.awaitingNewInput = options.saved?.awaiting_new_input === true;
        this.updatedAt = Math.floor(Date.now() / 1000);
        for (const turn of this.turns) if (turn.status === 'inProgress') turn.status = 'unknown';
        for (const receipt of this.receipts.values()) if (receipt.state === 'pending') receipt.state = 'uncertain';
    }
    emit(event) { this.options.event?.(event); }
    commit(turns, active) {
        this.options.persist?.({ turns, active_id: active?.id ?? null, receipts: [...this.receipts], awaiting_new_input: this.awaitingNewInput });
        this.turns = turns;
        this.active = active;
        this.updatedAt = Math.floor(Date.now() / 1000);
    }
    info() {
        if (!this.identity) throw failure('session_changed', 'Antigravity session has not registered');
        return { instance: this.instance, seq: this.options.sequence?.() ?? 0,
            session: { id: this.identity.conversationId, cwd: this.identity.workspacePaths[0], name: null,
                preview: this.turns.at(-1)?.user_text ?? null, updatedAt: this.updatedAt,
                status: this.active ? 'active' : 'idle', terminal: null }, capabilities: ['history', 'status',
                    ...(this.options.delivery ? ['prompt'] : []), ...(this.options.delivery?.stop ? ['stop'] : []),
                    ...(this.options.controls ? ['model', 'reasoning'] : []), ...(this.options.nativeNew ? ['new'] : [])] };
    }
    history(cursor, limit = 20) {
        const end = cursor == null ? this.turns.length : Number(cursor);
        if (!Number.isSafeInteger(end) || end < 0 || end > this.turns.length || !Number.isSafeInteger(limit) || limit < 1) {
            throw failure('invalid_request', 'Invalid history cursor or limit');
        }
        const size = Math.min(limit, 20), start = Math.max(0, end - size);
        return { turns: structuredClone(this.turns.slice(start, end)), older_cursor: start ? String(start) : null,
            newer_cursor: end < this.turns.length ? String(Math.min(this.turns.length, end + size)) : null };
    }
    queueState() {
        const uncertain = [...this.receipts].find(([, receipt]) => receipt.state === 'uncertain');
        return { items: [], paused: false, uncertain: uncertain ? { id: uncertain[0], text: uncertain[1].text } : null };
    }
    snapshot() { return { ...this.info(), turns: this.history().turns, queue: this.queueState() }; }
    clearInteraction() {
        if (!this.pendingInteraction) return;
        this.emit({ InteractionResolved: { session_id: this.identity.conversationId, request_id: this.pendingInteraction.request.rpc_id } });
        this.pendingInteraction = null;
    }
    replayInteraction() {
        if (this.pendingInteraction) this.emit({ InteractionRequested: this.pendingInteraction.request });
    }
    async pollInteraction() {
        if (this.closed || this.controlling || !this.options.interactions) return;
        if (!this.active) { this.clearInteraction(); return; }
        const prompt = await this.options.interactions.inspect();
        if (this.closed || this.controlling || !this.active) return;
        if (!prompt) { this.clearInteraction(); this.answeredFingerprint = null; return; }
        if (prompt.fingerprint === this.pendingInteraction?.prompt.fingerprint || prompt.fingerprint === this.answeredFingerprint) return;
        this.clearInteraction();
        const question = prompt.kind === 'user_input';
        const request = { rpc_id: randomUUID(), method: 'antigravity.terminal', session_id: this.identity.conversationId,
            turn_id: this.active.id, item_id: null, kind: prompt.kind, title: prompt.title, detail: prompt.detail,
            available_decisions: question ? [] : [...prompt.choices, 'cancel'],
            payload: question ? { rejectable: true, questions: [{ id: prompt.questionId, header: 'Antigravity question',
                question: prompt.title, multiple: prompt.multiple, custom: prompt.custom, required: true,
                options: (prompt.custom ? prompt.choices.slice(0, -1) : prompt.choices).map(label => ({ label })) }] }
                : { cancelable: true }, auto_resolution_ms: null };
        this.pendingInteraction = { prompt, request };
        this.replayInteraction();
    }
    async request(method, params = {}) {
        if (!this.identity) throw failure('session_changed', 'Antigravity session has not registered');
        if (this.closed) throw failure('session_changed', 'Antigravity bridge disconnected');
        if (this.retiring) throw failure('session_changed', 'Antigravity switched to a new conversation');
        if (method === 'info') return this.info();
        if (method === 'snapshot') return this.snapshot();
        if (method === 'history') return this.history(params.cursor, params.limit);
        if (method === 'terminal_input' && this.options.delivery?.draft) {
            if (this.active && params.clear == null) return { draft: null };
            if (this.controlling || this.active || this.waiters.size) throw failure('busy', 'Antigravity is busy');
            this.controlling = true;
            try { return { draft: await this.options.delivery.draft(params.clear) }; }
            finally { this.controlling = false; }
        }
        if (method === 'queue_state') return this.queueState();
        if (method === 'queue_resume') {
            if (this.queueState().uncertain) throw failure('delivery_uncertain', 'Inspect history and clear the uncertain delivery first');
            return { message: 'No pending deliveries' };
        }
        if (method === 'queue_clear') {
            if (this.active || this.waiters.size) throw failure('busy', 'Wait for the active turn or delivery acknowledgement');
            const previous = structuredClone([...this.receipts]);
            for (const receipt of this.receipts.values()) if (receipt.state === 'uncertain') receipt.state = 'cleared';
            try { this.commit(this.turns, this.active); }
            catch (error) { this.receipts = new Map(previous); throw error; }
            return { message: 'Uncertain delivery cleared; no message was cancelled or resent' };
        }
        if (method === 'stop' && this.options.delivery?.stop) return this.options.delivery.stop();
        if (method === 'interaction') {
            const pending = this.pendingInteraction;
            if (!pending || pending.request.rpc_id !== params.rpc_id) throw failure('invalid_request', 'Interaction is no longer pending');
            this.answeredFingerprint = pending.prompt.fingerprint;
            this.clearInteraction();
            return this.options.interactions.respond(pending.prompt, params.response);
        }
        if (method === 'command' && params.name === 'status') return { body: `Session: ${this.identity.conversationId}\nWorkspace: ${this.identity.cwd ?? this.identity.workspacePaths[0]}\nState: ${this.active ? 'active' : 'idle'}`, choices: [] };
        if (method === 'command' && params.name === 'new' && this.options.nativeNew) {
            if (this.controlling || this.waiters.size || this.queueState().uncertain) throw failure('busy', 'Resolve the pending delivery or terminal operation first');
            this.controlling = true;
            try {
                await this.options.nativeNew(() => Boolean(this.active));
                this.awaitingNewInput = true;
                this.commit(this.turns, this.active);
                return { body: 'New conversation ready. Send your next message.', choices: [] };
            } finally { this.controlling = false; }
        }
        if (method === 'command' && this.options.controls && ['model', 'reasoning'].includes(params.name)) {
            if (this.controlling || this.active || [...this.receipts.values()].some(receipt => ['pending', 'uncertain'].includes(receipt.state))) {
                throw failure('busy', 'Antigravity is busy or has an uncertain delivery');
            }
            this.controlling = true;
            try { return await this.options.controls.command(params.name, params.value); }
            finally { this.controlling = false; }
        }
        if (method !== 'prompt' || !this.options.delivery) throw failure('unsupported_method', `Unsupported Antigravity operation: ${method}`);
        const { request_id: id, text } = params;
        if (typeof id !== 'string' || !id || typeof text !== 'string' || !text.trim()) throw failure('invalid_request', 'request_id and text are required');
        const old = this.receipts.get(id);
        if (old) {
            if (old.text !== text) throw failure('invalid_request', 'request_id reused with different content');
            if (old.state === 'accepted') return { turn_id: old.turn_id };
            throw failure('delivery_uncertain', 'Delivery is pending or uncertain; do not resend');
        }
        if (this.controlling || this.active || [...this.receipts.values()].some(r => ['pending', 'uncertain'].includes(r.state))) throw failure('busy', 'Antigravity is busy or has an uncertain delivery');
        const baseline = this.readTranscript(this.identity.transcriptPath, this.identity.conversationId).at(-1)?.id;
        const receipt = { text, state: 'pending', baseline };
        this.receipts.set(id, receipt);
        try { this.commit(this.turns, this.active); } catch (error) { this.receipts.delete(id); throw error; }
        return new Promise((resolve, reject) => {
            const controller = new AbortController();
            const settleFailure = error => {
                const waiter = this.waiters.get(id);
                if (!waiter) return;
                clearTimeout(waiter.timer); this.waiters.delete(id); controller.abort();
                receipt.state = 'uncertain';
                if (error.notSent) this.receipts.delete(id);
                try { this.commit(this.turns, this.active); }
                catch (storageError) { this.receipts.set(id, receipt); reject(failure('delivery_uncertain', storageError.message)); return; }
                reject(error.notSent ? error : failure('delivery_uncertain', error.message));
            };
            const timer = setTimeout(() => settleFailure(new Error('Antigravity has not acknowledged this message')), this.options.ackTimeout ?? 7000);
            this.waiters.set(id, { resolve, reject, timer, controller, settleFailure });
            Promise.resolve().then(() => this.options.delivery.send({ request_id: id, text, signal: controller.signal })).catch(settleFailure);
        });
    }
    close() {
        this.closed = true;
        this.clearInteraction();
        for (const waiter of [...this.waiters.values()]) waiter.settleFailure(new Error('Antigravity bridge disconnected'));
    }
    acknowledgeHandoff(receipts) {
        for (const [id, waiter] of this.waiters) {
            const accepted = receipts.get(id), receipt = this.receipts.get(id);
            if (accepted?.state !== 'accepted' || accepted.text !== receipt?.text) continue;
            const previous = structuredClone(receipt);
            Object.assign(receipt, accepted);
            try { this.commit(this.turns, this.active); }
            catch (error) { Object.assign(receipt, previous); throw error; }
            clearTimeout(waiter.timer); this.waiters.delete(id);
            waiter.resolve({ turn_id: accepted.turn_id });
        }
    }
    hook(value) {
        if (!this.identity) {
            if (value.hook_event_name !== 'SessionStart') return;
            if (typeof value.conversationId !== 'string' || !value.conversationId ||
                typeof value.transcriptPath !== 'string' || !Array.isArray(value.workspacePaths) ||
                !value.workspacePaths.length || value.workspacePaths.some(p => typeof p !== 'string' || !p)) {
                throw failure('invalid_request', 'Invalid Antigravity hook identity');
            }
            this.identity = structuredClone(value);
            const saved = new Map(this.turns.map(turn => [turn.id, turn]));
            const native = this.readTranscript(value.transcriptPath, value.conversationId);
            this.turns = native.map(turn => saved.get(turn.id) ?? turn);
            for (const turn of saved.values()) if (!this.turns.some(value => value.id === turn.id)) this.turns.push(turn);
            return;
        }
        if (value.conversationId !== this.identity.conversationId) return;
        const transcript = this.readTranscript(this.identity.transcriptPath, value.conversationId);
        const native = value.agentix_turn_id ? transcript.find(turn => turn.id === value.agentix_turn_id) : transcript.at(-1);
        if (!native) return;
        if (value.hook_event_name === 'PreInvocation') {
            const invocation = JSON.stringify([native.id, value.invocationNum, value.initialNumSteps]);
            if (invocation !== this.invocation) {
                this.clearInteraction();
                this.answeredFingerprint = null;
                this.invocation = invocation;
            }
        }
        const index = this.turns.findIndex(turn => turn.id === native.id);
        let turns = structuredClone(this.turns), turn = index < 0 ? null : turns[index];
        const events = [], session_id = value.conversationId;
        if (value.hook_event_name === 'PreInvocation' && (!turn || (turn.status === 'unknown' && !this.active))) {
            if (this.active) throw failure('busy', 'Previous Antigravity turn has not settled');
            turn = { ...native, status: 'inProgress' };
            if (index < 0) turns.push(turn);
            else turns[index] = turn;
            events.push({ TurnStarted: { session_id, turn_id: turn.id } });
            events.push({ ItemCompleted: { session_id, turn_id: turn.id,
                item: { id: `${turn.id}:user`, kind: 'userMessage', text: turn.user_text, status: 'completed' } } });
        }
        if (!turn || (this.active?.id !== turn.id && !events.length)) return;
        if (['PostInvocation', 'Stop'].includes(value.hook_event_name)) {
            for (const usage of native.token_usage ?? []) this.options.usage?.(usage.id, usage);
        }
        const oldText = turn.agent_text;
        for (const item of native.items) {
            if (!turn.items.some(previous => previous.id === item.id && JSON.stringify(previous) === JSON.stringify(item))) {
                events.push({ ItemCompleted: { session_id, turn_id: turn.id, item } });
            }
        }
        turn.agent_text = native.agent_text;
        turn.items = native.items;
        turn.tools = native.tools;
        if (oldText !== turn.agent_text) events.push({ ItemCompleted: { session_id, turn_id: turn.id,
            item: { id: `${turn.id}:assistant`, kind: 'agentMessage', text: turn.agent_text, status: 'inProgress' } } });
        let active = turn;
        if (value.hook_event_name === 'Stop' && value.fullyIdle === true) {
            const reason = String(value.terminationReason ?? '').toLowerCase().replace(/^terminal_/, '');
            turn.status = value.error || ['error', 'max_steps_exceeded', 'max_invocations', 'max_forced_invocations', 'max_token_budget_exceeded'].includes(reason) ? 'failed'
                : ['cancelled', 'interrupted', 'custom_hook', 'user_canceled', 'user_cancelled'].includes(reason) ? 'interrupted' : 'completed';
            active = null;
            events.push({ ItemCompleted: { session_id, turn_id: turn.id,
                item: { id: `${turn.id}:assistant`, kind: 'agentMessage', text: turn.agent_text, status: turn.status } } });
            events.push({ TurnCompleted: { session_id, turn_id: turn.id, status: turn.status, error: value.error ?? null } });
        }
        const accepted = events.some(event => event.TurnStarted) && [...this.receipts].find(([, receipt]) =>
            ['pending', 'uncertain'].includes(receipt.state) && receipt.baseline !== turn.id && receipt.text === turn.user_text);
        const previous = accepted && structuredClone(accepted[1]);
        if (accepted) Object.assign(accepted[1], { state: 'accepted', turn_id: turn.id });
        try { this.commit(turns, active); }
        catch (error) { if (accepted) this.receipts.set(accepted[0], previous); throw error; }
        if (accepted) {
            const waiter = this.waiters.get(accepted[0]);
            if (waiter) { clearTimeout(waiter.timer); this.waiters.delete(accepted[0]); waiter.resolve({ turn_id: turn.id }); }
        }
        for (const event of events) this.emit(event);
    }
}
