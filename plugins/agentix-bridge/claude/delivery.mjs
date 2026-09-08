import { execFile } from 'node:child_process';
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
function run(command, args, input, signal) {
    return new Promise((resolve, reject) => {
        const child = execFile(command, args, { timeout: 2000, maxBuffer: 1024 * 1024, signal }, (error, stdout) => error ? reject(error) : resolve(stdout));
        child.stdin.on('error', () => {});
        child.stdin.end(input);
    });
}
export class RmuxDelivery {
    kind = 'rmux';
    constructor({ env = process.env, pid = process.ppid, run: execute = run, delay = sleep } = {}) {
        this.socket = env.TMUX?.replace(/,\d+,\d+$/, '');
        this.pane = env.TMUX_PANE;
        this.pid = pid; this.run = execute; this.delay = delay;
    }
    async check(signal, allowDraft = false) {
        const args = ['-S', this.socket];
        const state = (await this.run('rmux', [...args, 'display-message', '-p', '-t', this.pane,
            '#{pane_pid}|#{pane_current_command}|#{cursor_x}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}'], undefined, signal)).trim().split('|');
        const [root, command, x, y, mode, dead] = state;
        const processes = await this.run('ps', ['-A', '-o', 'pid=,ppid='], undefined, signal);
        const parents = new Map(processes.trim().split('\n').map(line => line.trim().split(/\s+/).map(Number)));
        let ancestor = this.pid; const seen = new Set();
        while (ancestor && ancestor !== Number(root) && !seen.has(ancestor)) { seen.add(ancestor); ancestor = parents.get(ancestor); }
        if (ancestor !== Number(root) || !parents.has(this.pid) || command !== 'claude' || mode !== '0' || dead !== '0') throw failure('busy', 'Original Claude process is not the active rmux pane');
        const [group, foreground] = (await this.run('ps', ['-p', String(this.pid), '-o', 'pgid=,tpgid='], undefined, signal)).trim().split(/\s+/).map(Number);
        if (!(group > 0) || group !== foreground) throw failure('busy', 'Original Claude process is not in the foreground');
        const screen = await this.run('rmux', [...args, 'capture-pane', '-p', '-t', this.pane], undefined, signal);
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
        const call = (args, input) => this.run('rmux', ['-S', this.socket, ...args], input, signal);
        try {
            if (!this.socket || !/^%\d+$/.test(this.pane ?? '')) throw failure('unsupported_method', 'Start Claude inside rmux to receive IM messages');
            if (typeof text !== 'string' || !text.trim() || /^[\s]*[\/!]/.test(text) || /[\x00-\x08\x0b-\x1f\x7f]/.test(text) || Buffer.byteLength(text) > 65536) throw failure('invalid_request', 'Send plain prompt text up to 64 KiB, without terminal controls or leading slash/bang commands');
            const empty = await this.check(signal, true);
            if (!empty) {
                // Cancel the entire idle draft, including multiline input. Never interrupt an empty prompt.
                signal?.throwIfAborted();
                await call(['send-keys', '-t', this.pane, 'C-c']);
                await this.delay(150);
                await this.check(signal);
            }
            loaded = true;
            await call(['load-buffer', '-b', buffer, '-'], text);
            await this.check(signal);
            signal?.throwIfAborted();
            pasted = true;
            await call(['paste-buffer', '-p', '-r', '-d', '-b', buffer, '-t', this.pane]);
            await this.delay(150);
            signal?.throwIfAborted();
            await call(['send-keys', '-t', this.pane, 'Enter']);
        } catch (error) {
            if (!pasted) error.notSent = true;
            throw error;
        } finally {
            if (loaded) await this.run('rmux', ['-S', this.socket, 'delete-buffer', '-b', buffer]).catch(() => {});
        }
    }
}
