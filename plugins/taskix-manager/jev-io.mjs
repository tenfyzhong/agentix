const deadlines = new WeakSet();

// One end-to-end budget shared by nested classifiers and preparation helpers.
// Racing cancellation also bounds injected transports that ignore AbortSignal.
export async function withDeadline(parent, operation) {
    parent?.throwIfAborted();
    if (parent && deadlines.has(parent)) {
        const result = await operation(parent);
        parent.throwIfAborted();
        return result;
    }
    const controller = new AbortController();
    const signal = parent ? AbortSignal.any([parent, controller.signal]) : controller.signal;
    deadlines.add(signal);
    let rejectAbort;
    const aborted = new Promise((_, reject) => { rejectAbort = () => reject(signal.reason); });
    signal.addEventListener("abort", rejectAbort, { once: true });
    const timer = setTimeout(() => controller.abort(new Error("Jev deadline exceeded")), 8000);
    try {
        return await Promise.race([aborted, operation(signal)]);
    } finally {
        clearTimeout(timer);
        signal.removeEventListener("abort", rejectAbort);
    }
}

// Bound decoded response bytes even when Content-Length is absent or compressed.
export async function readJson(response, signal) {
    signal.throwIfAborted();
    const limit = 1024 * 1024;
    if (Number(response.headers?.get("content-length")) > limit) {
        void response.body?.cancel().catch(() => {});
        throw new Error("Evaluation response too large");
    }
    // Injected test transports may expose only json(); native fetch always streams.
    if (!response.body?.getReader) return response.json();
    const reader = response.body.getReader(), chunks = [];
    let size = 0;
    const cancel = () => { void reader.cancel().catch(() => {}); };
    signal.addEventListener("abort", cancel, {once:true});
    try {
        while (true) {
            signal.throwIfAborted();
            const {done, value} = await reader.read();
            signal.throwIfAborted();
            if (done) break;
            size += value.byteLength;
            if (size > limit) throw new Error("Evaluation response too large");
            chunks.push(value);
        }
        return JSON.parse(Buffer.concat(chunks, size).toString("utf8"));
    } finally {
        signal.removeEventListener("abort", cancel);
        cancel();
    }
}
