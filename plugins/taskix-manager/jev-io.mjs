import * as http from "node:http";
import * as https from "node:https";
import { Readable } from "node:stream";

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

const [nodeMajor, nodeMinor] = process.versions.node.split(".").map(Number);
const supportsProxyAgent = nodeMajor > 24 || (nodeMajor === 24 && nodeMinor >= 5)
    || (nodeMajor === 22 && nodeMinor >= 21);

export async function requestDecisionJson(url, init, fetch, proxyEnv) {
    const target = new URL(url);
    const isHttps = target.protocol === "https:";
    const proxy = isHttps ? proxyEnv?.https_proxy?.trim() || proxyEnv?.HTTPS_PROXY?.trim()
        : proxyEnv?.http_proxy?.trim() || proxyEnv?.HTTP_PROXY?.trim();
    if (!proxy) {
        const response = await fetch(url, init);
        if (!response.ok) throw new Error("Evaluation unavailable");
        return readJson(response, init.signal);
    }
    if (!supportsProxyAgent || !["http:", "https:"].includes(new URL(proxy).protocol)) {
        throw new Error("Decision proxy unavailable");
    }
    // Configure only this request's agent. Other providers and host requests keep their transport.
    const transport = isHttps ? https : http;
    const proxySettings = {
        [isHttps ? "HTTPS_PROXY" : "HTTP_PROXY"]: proxy,
        NO_PROXY: proxyEnv.no_proxy?.trim() || proxyEnv.NO_PROXY?.trim(),
    };
    const agent = new transport.Agent({ proxyEnv: proxySettings });
    try {
        const response = await new Promise((resolve, reject) => {
            const request = transport.request(target, {
                agent,
                signal: init.signal,
                method: init.method,
                headers: {
                    ...init.headers,
                    "Accept-Encoding": "identity",
                    "Content-Length": Buffer.byteLength(init.body),
                },
            }, resolve);
            request.once("error", reject);
            request.end(init.body);
        });
        // The native client does not follow redirects.
        if (response.statusCode < 200 || response.statusCode >= 300) throw new Error("Evaluation unavailable");
        return await readJson({
            headers: new Headers(response.headers),
            body: Readable.toWeb(response),
        }, init.signal);
    } finally {
        agent.destroy();
    }
}
