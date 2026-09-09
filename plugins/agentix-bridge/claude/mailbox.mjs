import { mkdirSync, readFileSync, writeFileSync, renameSync, readdirSync, unlinkSync, realpathSync, watch } from 'node:fs';
import { join } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { homedir } from 'node:os';

export function hostKey(pid = process.ppid) {
    // Both MCP and exec-form hooks are direct children of Claude. Start time prevents PID reuse.
    const started = execFileSync('ps', ['-p', String(pid), '-o', 'lstart='], { encoding: 'utf8' }).trim();
    if (!started) throw new Error('Claude parent process is unavailable');
    return `${pid}-${started}`;
}
export const dataRoot = () => process.env.AGENTIX_CLAUDE_DATA_DIR ?? join(homedir(), '.local/share/agentix/claude');
export function atomicJson(path, value) {
    const temporary = `${path}.${randomUUID()}.tmp`;
    writeFileSync(temporary, JSON.stringify(value), { mode: 0o600 });
    renameSync(temporary, path);
}
export function readJson(path) {
    try { return JSON.parse(readFileSync(path, 'utf8')); }
    catch (error) { if (error.code === 'ENOENT') return null; throw error; }
}
export class Mailbox {
    constructor(root = dataRoot(), key = hostKey()) {
        this.path = join(root, 'hosts', createHash('sha256').update(key).digest('hex'));
        mkdirSync(this.path, { recursive: true, mode: 0o700 });
        // Native realpath expands Windows 8.3 names as well as symlinks.
        // Use one canonical path for publishing and watching libuv events.
        this.path = realpathSync.native(this.path);
    }
    identity() { return readJson(join(this.path, 'identity.json')); }
    publish(event) {
        if (typeof event.session_id !== 'string' || typeof event.cwd !== 'string' || typeof event.transcript_path !== 'string') throw new Error('Invalid Claude hook identity');
        if (event.hook_event_name === 'SessionStart') atomicJson(join(this.path, 'identity.json'), event);
        atomicJson(join(this.path, `${process.hrtime.bigint().toString().padStart(24, '0')}-${randomUUID()}.event.json`), event);
    }
    async consume(handle) {
        const names = readdirSync(this.path).filter(name => name.endsWith('.event.json')).sort();
        for (const name of names) {
            const path = join(this.path, name);
            await handle(readJson(path));
            unlinkSync(path);
        }
    }
    watch(wake) {
        let watcher, pending, closed = false;
        const stop = () => { closed = true; clearImmediate(pending); watcher?.close(); };
        try {
            watcher = watch(this.path, (_event, name) => {
                if (closed || pending || (name && name !== 'identity.json' && !String(name).endsWith('.event.json'))) return;
                pending = setImmediate(() => { pending = undefined; if (!closed) wake(); });
            });
            // The server retains a low-frequency polling fallback if watching is unavailable.
            watcher.on('error', stop);
        } catch { stop(); }
        return stop;
    }
}
