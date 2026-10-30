import { accessSync, constants } from 'node:fs';
import { delimiter, isAbsolute, join } from 'node:path';
import { failure } from '../protocol/errors.mjs';

export function rexCandidates(env, platform = process.platform) {
    const paths = (env.PATH ?? '').split(delimiter).filter(isAbsolute).map(path => join(path, 'rex'));
    if (platform === 'darwin') for (const root of ['/Applications', ...(env.HOME ? [join(env.HOME, 'Applications')] : [])]) {
        for (const app of ['Rex.app', 'Rex Beta.app']) paths.push(join(root, app, 'Contents/Helpers/Rex Server.rex/Contents/MacOS/rex'));
    }
    return paths;
}
const fail = message => failure('unsupported_method', message);
function localEndpoint(value) {
    const url = new URL(value);
    if (!value.startsWith('unix:///') || url.hostname || url.search || url.hash || url.pathname === '/') throw fail('Rex requires a local Unix socket endpoint');
    return value;
}

// Rex emits a screen redraw, not arbitrary application output. Reject unknown
// controls rather than guessing a cursor position and typing into another field.
export function decodeRexVT(input, rows, columns) {
    if (![rows, columns].every(Number.isSafeInteger) || rows < 1 || columns < 1 || rows > 1024 || columns > 4096 || rows * columns > 1048576) throw fail('Invalid Rex terminal size');
    const blank = () => Array(columns).fill(' ');
    const grid = Array.from({ length: rows }, blank);
    let x = 0, y = 0, cursor = false, glyphs;
    const segmenter = new Intl.Segmenter('en', { granularity: 'grapheme' });
    const linefeed = () => { if (++y === rows) { grid.shift(); grid.push(blank()); y--; } };
    for (let offset = 0; offset < input.length;) {
        if (input.startsWith('\x1b[', offset)) {
            const match = /^\x1b\[([0-9;]*)([A-Za-z])/.exec(input.slice(offset));
            if (!match) throw fail('Unsupported Rex VT sequence');
            const values = match[1].split(';').map(v => Number(v || 0));
            const count = Math.max(1, values[0]);
            switch (match[2]) {
                case 'H': case 'f': y = count - 1; x = Math.max(1, values[1] ?? 1) - 1; cursor = true; if (y >= rows || x >= columns) throw fail('Invalid Rex cursor'); break;
                case 'C': x = Math.min(columns - 1, x + count); break;
                case 'D': x = Math.max(0, x - count); break;
                case 'G': x = count - 1; if (x >= columns) throw fail('Invalid Rex cursor'); break;
                case 'X': grid[y].fill(' ', Math.min(x, columns), Math.min(columns, x + count)); break;
                case 'K': {
                    const ranges = [[Math.min(x, columns), columns], [0, Math.min(x + 1, columns)], [0, columns]];
                    if (!ranges[values[0]]) throw fail('Unsupported Rex erase');
                    grid[y].fill(' ', ...ranges[values[0]]); break;
                }
                case 'm': break;
                default: throw fail('Unsupported Rex VT sequence');
            }
            offset += match[0].length; glyphs = undefined; continue;
        }
        if (!glyphs) { const end = input.indexOf('\x1b', offset); glyphs = segmenter.segment(input.slice(offset, end < 0 ? input.length : end))[Symbol.iterator](); }
        const ch = glyphs.next().value?.segment;
        if (!ch) throw fail('Incomplete Rex grapheme');
        const cp = ch.codePointAt(0); offset += ch.length;
        if (ch === '\r\n') { x = 0; linefeed(); continue; }
        if (ch === '\r') { x = 0; continue; }
        if (ch === '\n') { linefeed(); continue; }
        if (ch === '\t') { x = Math.min(columns - 1, (Math.floor(x / 8) + 1) * 8); continue; }
        if (cp < 32 || (cp >= 127 && cp < 160)) throw fail('Unsupported Rex control');
        if (/^\p{Mark}+$/u.test(ch)) { if (x) { let p = Math.min(x, columns) - 1; while (p && grid[y][p] === '') p--; grid[y][p] += ch; } continue; }
        const wide = cp >= 0x1100 && (cp <= 0x115f || cp === 0x2329 || cp === 0x232a ||
            (cp >= 0x2e80 && cp <= 0xa4cf && cp !== 0x303f) || (cp >= 0xac00 && cp <= 0xd7a3) ||
            (cp >= 0xf900 && cp <= 0xfaff) || (cp >= 0xfe10 && cp <= 0xfe19) || (cp >= 0xfe30 && cp <= 0xfe6f) ||
            (cp >= 0xff00 && cp <= 0xff60) || (cp >= 0xffe0 && cp <= 0xffe6) || (cp >= 0x1f300 && cp <= 0x1faff) || cp >= 0x20000);
        const emoji = !ch.includes('\ufe0e') && (/\p{Emoji_Presentation}/u.test(ch) || ch.includes('\ufe0f') || ch.includes('\u200d'));
        const width = wide || emoji ? 2 : 1;
        if (width > columns) throw fail('Rex grapheme exceeds terminal width');
        if (x + width > columns) { x = 0; linefeed(); }
        grid[y][x] = ch; if (wide) grid[y][x + 1] = ''; x += width;
    }
    if (!cursor || x >= columns) throw fail('Incomplete Rex cursor snapshot');
    return { text: grid.map(row => row.join('').trimEnd()).join('\n'), x, y };
}

export class RexTerminalAdapter {
    kind = 'rex';
    constructor({ env = process.env, pid = process.ppid, run, command } = {}) {
        this.env = { ...env }; this.pid = pid; this.run = run;
        this.pane = env.REX_BLOCK; this.session = env.REX_SESSION;
        this.command = command ?? rexCandidates(env).find(path => { try { accessSync(path, constants.X_OK); return true; } catch { return false; } });
        this.buffers = new Map();
    }
    async connect(signal) {
        if (this.endpoint) return;
        if (!this.command || !this.session?.startsWith('session:') || !this.pane?.startsWith('block:')) throw fail('Start the agent inside a local Rex terminal');
        const args = ['--autostart=false', '--timeout', '2s'];
        if (this.env.REX_SERVER) args.push('-S', localEndpoint(this.env.REX_SERVER));
        const identity = JSON.parse(await this.run(this.command, [...args, 'whoami', '--json'], undefined, signal));
        this.endpoint = localEndpoint(identity.endpoint); this.socket = this.endpoint; this.host = identity.host_id;
        if (!this.host) { this.endpoint = undefined; throw fail('Missing Rex host identity'); }
    }
    async callJSON(args, signal) {
        await this.connect(signal);
        const output = await this.run(this.command, ['--autostart=false', '--timeout', '2s', '-S', this.endpoint, ...args], undefined, signal);
        return output.trim() ? JSON.parse(output) : {};
    }
    block(method, args = {}, signal) { return this.callJSON(['block', 'call', '-s', this.session, '-b', this.pane, method, JSON.stringify(args)], signal); }
    async verify(signal) {
        const inspected = await this.callJSON(['block', 'inspect', this.pane, '-s', this.session, '--json'], signal);
        if (inspected.block?.host_id !== this.host || inspected.block?.creator_name !== 'com.superlogical.terminal') throw fail('Rex block is not a local terminal');
        const process = await this.block('process', {}, signal);
        const root = process.child?.pid;
        if (!Number.isSafeInteger(root) || root <= 0 || process.last_exit != null) throw fail('Original Rex process exited');
        const parents = new Map((await this.run('ps', ['-A', '-o', 'pid=,ppid='], undefined, signal)).trim().split('\n').map(line => line.trim().split(/\s+/).map(Number)));
        let ancestor = this.pid; const seen = new Set();
        while (ancestor && ancestor !== root && !seen.has(ancestor)) { seen.add(ancestor); ancestor = parents.get(ancestor); }
        if (ancestor !== root || !parents.has(this.pid)) throw failure('busy', 'Original agent does not belong to this Rex block');
        return process;
    }
    async foreground(signal) {
        await this.verify(signal);
        const [group, foreground] = (await this.run('ps', ['-p', String(this.pid), '-o', 'pgid=,tpgid='], undefined, signal)).trim().split(/\s+/).map(Number);
        if (!(group > 0) || group !== foreground) throw failure('busy', 'Original agent process is not in the foreground');
    }
    async inspect(signal) {
        const process = await this.verify(signal); await this.foreground(signal);
        const size = await this.block('size', {}, signal);
        let result = await this.block('format', { format: 'vt', extra_cursor: true, max_bytes: 65536 }, signal);
        let content = result.content, offset = Buffer.byteLength(content); const deadline = Date.now() + 5000;
        while (result.id !== undefined) {
            if (offset > 64 * 1024 * 1024 || Date.now() >= deadline) throw fail('Rex capture exceeded its read budget');
            result = await this.block('blob_read', { id: result.id, offset, max_bytes: 65536 }, signal);
            if (typeof result.data !== 'string' || !result.data.length) throw fail('Incomplete Rex capture');
            offset += Buffer.byteLength(result.data); content += result.data;
            if (result.eof) break;
        }
        const snapshot = decodeRexVT(content, size.rows, size.columns);
        const after = await this.block('size', {}, signal);
        if (after.rows !== size.rows || after.columns !== size.columns) throw failure('busy', 'Rex terminal resized during capture');
        await this.foreground(signal); this.screen = snapshot.text;
        return [String(process.child.pid), process.foreground?.name ?? '', String(snapshot.x), String(snapshot.y), '0', '0'];
    }
    async capture() { if (this.screen === undefined) throw fail('Inspect Rex before capture'); return this.screen; }
    async sendKey(key, signal) {
        await this.foreground(signal);
        const mapped = { 'C-c': 'ctrl+c', Enter: 'enter', Escape: 'escape', Up: 'up', Down: 'down' }[key] ?? key;
        await this.callJSON(['send-key', '-s', this.session, '-b', this.pane, mapped], signal);
    }
    async sendText(text, signal) { await this.foreground(signal); await this.block('write', { data: text }, signal); }
    async paste(text, signal) {
        await this.foreground(signal);
        await this.callJSON(['api', 'call', 'block.event', JSON.stringify({ session_id: this.session, block_id: this.pane, event: { type: 'paste', payload: { text } } })], signal);
    }
    async loadPaste(id, text) { this.buffers.set(id, text); }
    async pasteLoaded(id, signal) { const text = this.buffers.get(id); if (typeof text !== 'string') throw fail('Missing Rex paste'); await this.paste(text, signal); this.buffers.delete(id); }
    async clearPaste(id) { this.buffers.delete(id); }
}
