import { execFile } from 'node:child_process';
import { failure } from '../protocol/errors.mjs';

function run(command, args, input, signal) {
    return new Promise((resolve, reject) => {
        const child = execFile(command, args, { timeout: 2000, maxBuffer: 1024 * 1024, signal }, (error, stdout) => error ? reject(error) : resolve(stdout));
        child.stdin.on('error', () => {});
        child.stdin.end(input);
    });
}

// Both drivers speak the same terminal command vocabulary. Keep server selection
// and read-only process verification here, outside Claude's input workflow.
export class TerminalAdapter {
    constructor(kind, { env = process.env, pid = process.ppid, run: execute = run } = {}) {
        if (!['rmux', 'tmux'].includes(kind)) throw failure('invalid_request', 'Invalid multiplexer configuration');
        this.kind = kind;
        this.socket = env.TMUX?.replace(/,\d+,\d+$/, '');
        this.pane = env.TMUX_PANE;
        this.pid = pid;
        this.run = execute;
    }
    call(args, input, signal) {
        if (!this.socket?.startsWith('/') || !/^%\d+$/.test(this.pane ?? '')) {
            throw failure('unsupported_method', 'Start Claude inside rmux or tmux to receive IM messages');
        }
        return this.run(this.kind, ['-S', this.socket, ...args], input, signal);
    }
    async inspect(signal) {
        const state = (await this.call(['display-message', '-p', '-t', this.pane,
            '#{pane_pid}|#{pane_current_command}|#{cursor_x}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}'], undefined, signal)).trim().split('|');
        const root = Number(state[0]);
        const processes = await this.run('ps', ['-A', '-o', 'pid=,ppid='], undefined, signal);
        const parents = new Map(processes.trim().split('\n').map(line => line.trim().split(/\s+/).map(Number)));
        let ancestor = this.pid; const seen = new Set();
        while (ancestor && ancestor !== root && !seen.has(ancestor)) { seen.add(ancestor); ancestor = parents.get(ancestor); }
        if (state.length !== 6 || !Number.isSafeInteger(root) || root <= 0 || ancestor !== root || !parents.has(this.pid)) {
            throw failure('busy', 'Original Claude process does not belong to this terminal pane');
        }
        return state;
    }
    async foreground(signal) {
        const [group, foreground] = (await this.run('ps', ['-p', String(this.pid), '-o', 'pgid=,tpgid='], undefined, signal)).trim().split(/\s+/).map(Number);
        if (!(group > 0) || group !== foreground) throw failure('busy', 'Original Claude process is not in the foreground');
    }
}

export async function detectTerminal(options, signal) {
    const candidates = await Promise.all(['rmux', 'tmux'].map(async kind => {
        const adapter = new TerminalAdapter(kind, options);
        try { await adapter.inspect(signal); return adapter; } catch { return null; }
    }));
    signal?.throwIfAborted();
    const matches = candidates.filter(Boolean);
    if (matches.length !== 1) throw failure('unsupported_method', 'Cannot uniquely verify the original rmux or tmux pane');
    return matches[0];
}
