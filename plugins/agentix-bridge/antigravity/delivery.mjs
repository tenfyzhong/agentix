import { randomUUID } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { TerminalAdapter, detectTerminal } from '../claude/terminal.mjs';
import { failure } from '../protocol/errors.mjs';

/** Native agy input, using shared multiplexer drivers and agy-specific readiness checks. */
export class AntigravityDelivery {
    constructor({ resolve, delay = sleep, ...options } = {}) {
        this.options = options;
        this.delay = delay;
        this.resolve = resolve ?? (signal => this.terminal ?? detectTerminal(options, signal));
    }
    configure(result) {
        const kind = result?.multiplexer?.kind;
        this.terminal = kind ? new TerminalAdapter(kind, this.options) : undefined;
    }
    async verify(terminal, signal) {
        const state = await terminal.inspect(signal);
        if (state[1] !== 'agy' || state[4] !== '0' || state[5] !== '0') throw failure('busy', 'Original Antigravity process is not the active terminal pane');
        await terminal.foreground(signal);
        return state;
    }
    async check(terminal, signal, allowDraft = false) {
        const [, , x, y] = await this.verify(terminal, signal);
        const screen = await terminal.capture(signal), lines = screen.split('\n'), row = Number(y);
        if (!/^(?:-- INSERT -- )?\? for shortcuts[^\S\n]{2,}\S[^\n]*$/m.test(screen) &&
            !(allowDraft && /^-- INSERT --[^\S\n]{2,}\S[^\n]*$/m.test(screen))) {
            throw failure('busy', 'Antigravity must finish initialization and be ready for input');
        }
        const border = line => /^─{3,}\s*$/.test(line ?? '');
        let top = row - 1, bottom = row + 1;
        while (top >= 0 && !border(lines[top])) top--;
        while (bottom < lines.length && !border(lines[bottom])) bottom++;
        if (!Number.isInteger(row) || top < 0 || bottom >= lines.length || !/^>(?:\s|$)/.test(lines[top + 1] ?? '') ||
            !Number.isInteger(Number(x)) || Number(x) < (row === top + 1 ? 2 : 0) ||
            /-- (?:NORMAL|VISUAL) --/.test(screen)) {
            throw failure('busy', 'Antigravity input must be empty and ready; dismiss dialogs and use insert mode');
        }
        const empty = bottom === top + 2 && /^>\s*$/.test(lines[top + 1]) && Number(x) === 2;
        if (!empty && !allowDraft) throw failure('busy', 'Confirm clearing the existing terminal draft in IM before sending');
        return empty ? null : lines.slice(top + 1, bottom).map(line => line.slice(2)).join('\n');
    }
    async draft(expected) {
        const terminal = await this.resolve();
        const current = await this.check(terminal, undefined, true);
        if (expected == null || current !== expected) return current;
        if (await this.check(terminal, undefined, true) !== expected) throw failure('busy', 'Antigravity draft changed before clearing');
        if (current.includes('\n')) throw failure('busy', 'Clear the multiline Antigravity draft in the terminal before sending');
        await terminal.sendKey('C-e');
        await this.verify(terminal);
        await terminal.sendKey('C-u');
        for (let attempt = 0; attempt < 10; attempt++) {
            await this.delay(100);
            try { await this.check(terminal); return null; }
            catch (error) { if (attempt === 9 || error.bridgeCode !== 'busy') throw error; }
        }
    }
    async stop() {
        const terminal = await this.resolve();
        await this.verify(terminal);
        await terminal.sendKey('Escape');
        return null;
    }
    async send({ text, signal }) {
        let pasted = false, loaded = false, terminal;
        const buffer = `agentix-agy-${randomUUID()}`;
        try {
            if (typeof text !== 'string' || !text.trim() || /^\s*[\/!]/.test(text) || /[\x00-\x08\x0b-\x1f\x7f]/.test(text) || Buffer.byteLength(text) > 65536) {
                throw failure('invalid_request', 'Send plain prompt text up to 64 KiB without terminal controls or leading slash/bang commands');
            }
            terminal = await this.resolve(signal);
            await this.check(terminal, signal);
            loaded = true;
            await terminal.loadPaste(buffer, text, signal);
            await this.check(terminal, signal);
            signal?.throwIfAborted();
            pasted = true;
            await terminal.pasteLoaded(buffer, signal);
            await this.delay(150);
            signal?.throwIfAborted();
            await terminal.sendKey('Enter', signal);
        } catch (error) {
            if (!pasted) error.notSent = true;
            throw error;
        } finally {
            if (loaded) await terminal.clearPaste(buffer).catch(() => {});
        }
    }
}
