/** Bounded live-message accounting. Historical snapshots never feed this accumulator. */
export class ObservedUsage {
    total = {};
    messages = new Map();
    record(id, usage) {
        if (typeof id !== 'string' || !id || !usage) return false;
        const previous = this.messages.get(id) ?? {};
        let changed = false;
        for (const kind of ['input', 'output', 'cached']) {
            const count = usage[kind];
            if (!Number.isSafeInteger(count) || count < 0) continue;
            const old = previous[kind] ?? 0;
            if (count < old || !Number.isSafeInteger((this.total[kind] ?? 0) + count - old)) continue;
            if (previous[kind] === undefined || count > old) changed = true;
            this.total[kind] = (this.total[kind] ?? 0) + count - old;
            previous[kind] = count;
        }
        if (Object.keys(previous).length) this.messages.set(id, previous);
        while (this.messages.size > 4096) this.messages.delete(this.messages.keys().next().value);
        return changed;
    }
}
