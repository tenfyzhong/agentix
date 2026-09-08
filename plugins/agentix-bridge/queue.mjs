import { failure } from './protocol/errors.mjs';

/** Durable delivery state. The host owns execution; this class never sends a prompt. */
export class DurableQueue {
    #queue;
    #head = 0;
    #paused;
    #inflight;
    #uncertain;
    #receipts;
    #persist;
    #requests;
    #failures;
    constructor(saved = {}, persist = () => {}) {
        const state = structuredClone(saved);
        this.#queue = state.queue ?? [];
        this.#inflight = state.inflight ?? null;
        this.#uncertain = state.uncertain ?? this.#inflight;
        this.#paused = Boolean(state.paused || this.#uncertain);
        this.#receipts = new Map(state.receipts ?? []);
        this.#requests = new Map(state.requests ?? []);
        this.#failures = new Map(state.failures ?? []);
        this.#persist = persist;
    }
    state() {
        return structuredClone({ queue: this.#queue.slice(this.#head), paused: this.#paused, inflight: this.#inflight,
            uncertain: this.#uncertain, receipts: [...this.#receipts], requests: [...this.#requests], failures: [...this.#failures] });
    }
    view() { return structuredClone({ items: this.#queue.slice(this.#head), paused: this.#paused, uncertain: this.#uncertain }); }
    receipt(id, intent) {
        const original = this.#requests.get(id);
        if (original && intent && (original.method !== intent.method || original.text !== intent.text)) {
            throw failure('invalid_request', 'request_id already belongs to a different request');
        }
        const rejected = this.#failures.get(id);
        if (rejected) throw failure(rejected.code, rejected.message);
        return structuredClone(this.#receipts.get(id));
    }
    /** Replay old checkpoints and incremental records before applying crash recovery. */
    static restore(records) {
        let queue = new DurableQueue();
        for (const record of records) {
            if (record.queue_delta) queue.#apply(structuredClone(record));
            else if (Array.isArray(record.queue)) {
                queue = new DurableQueue(record);
                queue.#inflight = structuredClone(record.inflight ?? null);
                queue.#uncertain = structuredClone(record.uncertain ?? null);
                queue.#paused = Boolean(record.paused);
            }
        }
        return queue.state();
    }
    record() {
        return structuredClone({ queue_delta: { op: 'state' }, paused: this.#paused,
            inflight: this.#inflight, uncertain: this.#uncertain });
    }
    #commit(delta, changes = {}) {
        const record = { ...this.record(), ...structuredClone(changes), queue_delta: structuredClone(delta) };
        // A failed append must leave both receipts and delivery ownership unchanged.
        this.#persist(structuredClone(record));
        this.#apply(record);
    }
    #apply(record) {
        const delta = record.queue_delta;
        if (delta.op === 'enqueue' && !this.#receipts.has(delta.item.id)) {
            this.#queue.push(delta.item);
            this.#receipts.set(delta.item.id, delta.item);
            this.#requests.set(delta.item.id, { method: 'queue', text: delta.item.text });
        } else if (delta.op === 'claim' && this.#queue[this.#head]?.id === delta.id) {
            this.#head++;
            if (this.#head >= 256 && this.#head * 2 >= this.#queue.length) {
                this.#queue = this.#queue.slice(this.#head); this.#head = 0;
            }
        } else if (delta.op === 'remember') {
            this.#receipts.set(delta.id, delta.result);
            if (delta.intent) this.#requests.set(delta.id, delta.intent);
        } else if (delta.op === 'fail') this.#failures.set(delta.id, delta.error);
        else if (delta.op === 'clear') { this.#queue = []; this.#head = 0; }
        this.#paused = record.paused;
        this.#inflight = record.inflight;
        this.#uncertain = record.uncertain;
    }
    enqueue(id, text) {
        if (typeof text !== 'string' || !text.trim() || typeof id !== 'string' || !id) {
            throw failure('invalid_request', 'Prompt text and request_id are required');
        }
        const intent = { method: 'queue', text };
        const receipt = this.receipt(id, intent);
        if (receipt) return receipt;
        const item = { id, text };
        this.#commit({ op: 'enqueue', item });
        return structuredClone(item);
    }
    peek() {
        return this.#paused || this.#inflight ? null : structuredClone(this.#queue[this.#head] ?? null);
    }
    claim() {
        const item = this.peek();
        if (!item) return null;
        this.#commit({ op: 'claim', id: item.id }, { inflight: item });
        return item;
    }
    remember(id, result, delivery = null, intent) {
        this.#commit({ op: 'remember', id, result, intent }, delivery ? { inflight: delivery } : {});
    }
    fail(id, error, uncertain = false) {
        this.#commit({ op: 'fail', id, error: { code: error.bridgeCode ?? 'host_error', message: error.message } },
            uncertain ? { uncertain: this.#inflight, paused: true } : {});
    }
    finish(interrupted = false) {
        this.#commit({ op: 'state' }, { inflight: null, paused: this.#paused || interrupted });
    }
    markUncertain() {
        this.#commit({ op: 'state' }, { uncertain: this.#inflight, paused: true });
    }
    pause() { this.#commit({ op: 'state' }, { paused: true }); }
    resume() {
        if (this.#uncertain) throw failure('delivery_uncertain', 'A previous delivery is uncertain. Check its history, then clear the queue before submitting again.');
        this.#commit({ op: 'state' }, { paused: false });
    }
    clear(idle) {
        this.#commit({ op: 'clear' }, { uncertain: null, paused: false, inflight: idle ? null : this.#inflight });
    }
}
