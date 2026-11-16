import { execFileSync } from 'node:child_process';

const cache = new Map();
// Decrypt only when a decision needs a credential, never during env export or CLI calls.
export function passKey(entry, env, timeout = 120_000) {
    if (typeof entry !== 'string' || !entry.trim() || entry.startsWith('-') || entry.startsWith('/') || entry.split('/').includes('..') || /[\x00-\x1f\x7f]/.test(entry)) {
        throw new Error('Invalid pass entry');
    }
    const identity = JSON.stringify([entry, ...['HOME','GNUPGHOME','PASSWORD_STORE_DIR','PATH'].map(key => env[key])]);
    if (cache.has(identity)) return cache.get(identity);
    let output;
    try {
        output = execFileSync('pass', ['show', '--', entry], {
            env, encoding:'utf8', timeout, killSignal:'SIGKILL', maxBuffer:64 * 1024,
            stdio:['ignore','pipe','ignore'], windowsHide:true,
        });
    } catch {
        // Child errors carry stdout/stderr; never retain their cause or output.
        throw new Error('pass decryption failed or timed out; configure pinentry or use a direct API key');
    }
    const key = output.split('\n', 1)[0].replace(/\r$/, '');
    if (!key.trim() || /[^\x20-\x7e]/.test(key)) throw new Error('pass returned an invalid API key');
    cache.set(identity, key);
    return key;
}
