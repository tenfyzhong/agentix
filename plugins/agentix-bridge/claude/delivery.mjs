import { TerminalAdapter, detectTerminal } from './terminal.mjs';
import { randomUUID } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { failure } from '../protocol/errors.mjs';

// A delivery submits text; only the host acknowledgement confirms receipt.
export class ChannelDelivery {
    kind = 'channel';
    constructor(notify) { this.notify = notify; }
    send({ request_id, text }) {
        return this.notify({ method: 'notifications/claude/channel', params: { content: text, meta: { request_id } } });
    }
}
export function deliveryMode(value = 'auto') {
    if (!['auto', 'multiplexer', 'channel'].includes(value)) throw failure('invalid_request', 'AGENTIX_CLAUDE_DELIVERY must be auto, multiplexer, or channel');
    return value;
}

export class TerminalDelivery {
    kind = 'multiplexer';
    usesPromptHook = true;
    constructor({ mode = 'auto', delay = sleep, ...options } = {}) {
        this.mode = deliveryMode(mode);
        if (this.mode === 'channel') throw failure('invalid_request', 'Use channel delivery for channel mode');
        this.options = { ...options, env: { ...(options.env ?? process.env) }, pid: options.pid ?? process.ppid };
        this.delay = delay;
    }
    configure(result) {
        this.configured = undefined;
        if (this.mode !== 'multiplexer') return;
        const kind = result?.multiplexer?.kind;
        if (!['rmux', 'tmux'].includes(kind)) throw failure('invalid_request', 'Bridge registration is missing valid multiplexer configuration');
        this.configured = new TerminalAdapter(kind, this.options);
    }
    async resolve(signal) {
        if (this.mode === 'auto') return detectTerminal(this.options, signal);
        if (!this.configured) throw failure('unsupported_method', 'Bridge multiplexer configuration is not available');
        return this.configured;
    }
    async check(terminal, signal, allowDraft = false) {
        const [, command, x, y, mode, dead] = await terminal.inspect(signal);
        if (command !== 'claude' || mode !== '0' || dead !== '0') throw failure('busy', 'Original Claude process is not the active terminal pane');
        await terminal.foreground(signal);
        const screen = await terminal.call(['capture-pane', '-p', '-t', terminal.pane], undefined, signal);
        const lines = screen.split('\n'), row = Number(y);
        const border = line => /^─{3,}\s*$/.test(line ?? '');
        let top = row - 1, bottom = row + 1;
        while (top >= 0 && !border(lines[top])) top--;
        while (bottom < lines.length && !border(lines[bottom])) bottom++;
        const prompt = lines[top + 1] ?? '';
        if (!Number.isInteger(row) || top < 0 || bottom >= lines.length || !/^❯(?:\s|$)/.test(prompt) ||
            !Number.isInteger(Number(x)) || Number(x) < (row === top + 1 ? 2 : 0) ||
            /-- (?:NORMAL|VISUAL) --/.test(screen)) throw failure('busy', 'Claude input must be ready; dismiss dialogs and use insert mode');
        const empty = bottom === top + 2 && /^❯\s*$/.test(prompt) && Number(x) === 2;
        if (!allowDraft && !empty) throw failure('busy', 'Claude input was not cleared; no IM text was pasted');
        return empty;
    }
    async send({ text, signal }) {
        let pasted = false, loaded = false;
        const buffer = `agentix-${randomUUID()}`;
        let terminal;
        const call = (args, input) => terminal.call(args, input, signal);
        try {
            if (typeof text !== 'string' || !text.trim() || /^[\s]*[\/!]/.test(text) || /[\x00-\x08\x0b-\x1f\x7f]/.test(text) || Buffer.byteLength(text) > 65536) throw failure('invalid_request', 'Send plain prompt text up to 64 KiB, without terminal controls or leading slash/bang commands');
            terminal = await this.resolve(signal);
            const empty = await this.check(terminal, signal, true);
            if (!empty) {
                // Cancel the entire idle draft, including multiline input. Never interrupt an empty prompt.
                signal?.throwIfAborted();
                await call(['send-keys', '-t', terminal.pane, 'C-c']);
                await this.delay(150);
                await this.check(terminal, signal);
            }
            loaded = true;
            await call(['load-buffer', '-b', buffer, '-'], text);
            await this.check(terminal, signal);
            signal?.throwIfAborted();
            pasted = true;
            await call(['paste-buffer', '-p', '-r', '-d', '-b', buffer, '-t', terminal.pane]);
            await this.delay(150);
            signal?.throwIfAborted();
            await call(['send-keys', '-t', terminal.pane, 'Enter']);
        } catch (error) {
            if (!pasted) error.notSent = true;
            throw error;
        } finally {
            if (loaded) await terminal.call(['delete-buffer', '-b', buffer]).catch(() => {});
        }
    }
}
