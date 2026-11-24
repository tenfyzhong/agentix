import { spawn, spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const cache = new Map();
const outputLimit = 64 * 1024;
const script = fileURLToPath(import.meta.url);

// Decrypt only when a decision needs a credential, never during env export or CLI calls.
export function passKey(entry, env, timeout = 120_000) {
    if (typeof entry !== 'string' || !entry.trim() || entry.startsWith('-') || entry.startsWith('/') || entry.split('/').includes('..') || /[\x00-\x1f\x7f]/.test(entry)) {
        throw new Error('Invalid pass entry');
    }
    const identity = JSON.stringify([entry, ...['HOME','GNUPGHOME','PASSWORD_STORE_DIR','PATH'].map(key => env[key])]);
    if (cache.has(identity)) return cache.get(identity);
    const options = { encoding: 'utf8', maxBuffer: outputLimit, windowsHide: true };
    const result = process.platform === 'win32'
        ? spawnSync('pass', ['show', '--', entry], {
            ...options, env, timeout, killSignal: 'SIGKILL', stdio: ['ignore', 'pipe', 'ignore'],
        })
        // A host may kill its hook before the decryption deadline. The detached
        // supervisor survives that signal long enough to clean up the pass group.
        : spawnSync(process.execPath, [script, String(process.pid), String(timeout), entry], {
            ...options, env: {}, input: JSON.stringify(env), stdio: ['pipe', 'pipe', 'ignore'], detached: true,
        });
    if (result.error || result.status !== 0) {
        throw new Error('pass decryption failed or timed out; configure pinentry or use a direct API key');
    }
    const key = result.stdout.split('\n', 1)[0].replace(/\r$/, '');
    if (!key.trim() || /[^\x20-\x7e]/.test(key)) throw new Error('pass returned an invalid API key');
    cache.set(identity, key);
    return key;
}

function supervisePass(caller, timeout, entry) {
    const env = JSON.parse(readFileSync(0, 'utf8'));
    if (process.ppid !== caller) {
        process.exitCode = 1;
        return;
    }
    const child = spawn('pass', ['show', '--', entry], {
        env, stdio: ['ignore', 'pipe', 'ignore'], detached: true,
    });
    const chunks = [];
    let size = 0;
    let failed = false;
    const stop = () => {
        if (failed) return;
        failed = true;
        child.stdout.destroy();
        if (child.pid !== undefined) {
            try { process.kill(-child.pid, 'SIGKILL'); }
            catch (error) { if (error.code !== 'ESRCH') throw error; }
        }
    };
    const deadline = setTimeout(stop, timeout);
    const callerWatch = setInterval(() => {
        try { process.kill(caller, 0); }
        catch { stop(); }
    }, 100);
    child.stdout.on('data', chunk => {
        if (failed) return;
        size += chunk.length;
        if (size > outputLimit) stop();
        else chunks.push(chunk);
    });
    child.on('error', stop);
    child.on('close', code => {
        clearTimeout(deadline);
        clearInterval(callerWatch);
        if (failed || code !== 0) process.exitCode = 1;
        else process.stdout.write(Buffer.concat(chunks, size));
    });
}

if (process.argv[1] === script) {
    const [, , caller, timeout, entry] = process.argv;
    supervisePass(Number(caller), Number(timeout), entry);
}
