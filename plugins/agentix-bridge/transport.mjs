import net from 'node:net';
import { FrameDecoder } from './framing.mjs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { validate } from './protocol/validate.mjs';
import { failure } from './protocol/errors.mjs';

import { PROTOCOL_VERSION, MAX_FRAME_BYTES } from './protocol/constants.mjs';
export { PROTOCOL_VERSION };
export const MAX_FRAME = MAX_FRAME_BYTES;

function connectionOptions(endpoint) {
    if (endpoint.startsWith('unix://') && endpoint.length > 7) {
        return { path: endpoint.slice(7).replace(/^~(?=\/)/, homedir()) };
    }
    if (endpoint.startsWith('tcp://')) {
        try {
            const url = new URL(endpoint);
            if (url.hostname && Number(url.port) > 0 && !url.username && !url.password
                && !url.pathname && !url.search && !url.hash) {
                return { host: url.hostname.replace(/^\[|\]$/g, ''), port: Number(url.port) };
            }
        } catch { /* Report the same configuration error for all malformed URLs. */ }
    }
    throw new Error('Invalid Agentix control endpoint: expected unix://path or tcp://host:port');
}

/** Outbound transport only. Session state and command execution stay with the host. */
export class BridgeTransport {
    #options;
    #record;
    #active = false;
    #sequence = 0;
    #clients = new Set();
    #timer;
    constructor(options) {
        this.#options = { ...options, reconnectDelay: options.reconnectDelay ?? 1000,
            endpoint: options.endpoint ?? process.env.AGENTIX_CONTROL_ENDPOINT ?? `unix://${join(homedir(), '.local/share/agentix/control.sock')}` };
        this.#options.connection = connectionOptions(this.#options.endpoint);
    }
    get sequence() { return this.#sequence; }
    open(record) {
        this.#record = record; this.#sequence = 0; this.#active = true;
        this.#connect(record.instance);
    }
    #send(socket, frame) {
        let encoded = JSON.stringify(frame) + '\n';
        if (Buffer.byteLength(encoded) > MAX_FRAME) {
            if (typeof frame.id !== 'string' || typeof frame.ok !== 'boolean') { socket.destroy(); return; }
            encoded = JSON.stringify({ id: frame.id, ok: false, code: 'frame_too_large', error: 'Bridge response exceeds maximum frame size' }) + '\n';
        }
        if (!socket.destroyed && Buffer.byteLength(encoded) <= MAX_FRAME && socket.writableLength < 8 * MAX_FRAME) socket.write(encoded);
        else socket.destroy();
    }
    event(value) {
        if (![...this.#clients].some(client => client.registered)) return;
        const frame = { instance: this.#record.instance, seq: ++this.#sequence, event: value };
        for (const client of this.#clients) if (client.registered) this.#send(client, frame);
    }
    async close(flush = false) {
        this.#active = false; clearTimeout(this.#timer);
        if (flush) {
            await Promise.race([
                Promise.all([...this.#clients].map(socket => new Promise(resolve => { socket.once('close', resolve); socket.end(resolve); }))),
                new Promise(resolve => setTimeout(resolve, 100)),
            ]);
        }
        for (const socket of this.#clients) socket.destroy();
        this.#clients.clear();
    }
    #retry(incarnation) {
        if (!this.#active || this.#record.instance !== incarnation) return;
        clearTimeout(this.#timer);
        this.#timer = setTimeout(() => this.#connect(incarnation), this.#options.reconnectDelay);
        this.#timer.unref();
    }
    async #handle(socket, frame, incarnation) {
        if (!this.#active || this.#record.instance !== incarnation || socket.destroyed) return;
        if (typeof frame.id !== 'string' || typeof frame.method !== 'string') return;
        try {
            if (!socket.registered) throw failure('invalid_request', 'Bridge registration required');
            if (!validate('Request', frame)) throw failure('invalid_request', 'Invalid request');
            const result = await this.#options.handle(frame.method, frame.params);
            this.#send(socket, { id: frame.id, ok: true, result });
        } catch (error) {
            this.#send(socket, { id: frame.id, ok: false, error: error.message, code: error.bridgeCode ?? 'host_error' });
        }
    }
    #connect(incarnation) {
        try {
            if (!this.#active || this.#record.instance !== incarnation) return;
            const socket = net.connect(this.#options.connection);
            this.#clients.add(socket);
            const deadline = setTimeout(() => socket.destroy(), 3000); deadline.unref();
            socket.on('error', () => {});
            socket.on('close', () => { clearTimeout(deadline); this.#clients.delete(socket); this.#retry(incarnation); });
            socket.on('connect', () => {
                if (!this.#active || this.#record.instance !== incarnation) return socket.destroy();
                this.#send(socket, { id: 'register', method: 'register', params: { ...this.#record, version: PROTOCOL_VERSION, snapshot: this.#options.snapshot() } });
            });
            const decoder = new FrameDecoder();
            socket.on('data', chunk => {
                let lines;
                try { lines = decoder.push(chunk); } catch { socket.destroy(); return; }
                for (const line of lines) {
                    let frame;
                    try { frame = JSON.parse(line.toString('utf8')); } catch { socket.destroy(); return; }
                    if (frame.id === 'register') {
                        if (frame.ok !== true) return socket.destroy();
                        try { this.#options.registered?.(frame.result); } catch (error) {
                            this.#options.registrationError?.(error); socket.destroy(); return;
                        }
                        socket.registered = true; clearTimeout(deadline); continue;
                    }
                    this.#options.dispatch(() => this.#handle(socket, frame, incarnation), frame.method).catch(() => socket.destroy());
                }
            });
        } catch { this.#retry(incarnation); }
    }
}
