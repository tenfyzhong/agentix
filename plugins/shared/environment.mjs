import { parseEnv } from 'node:util';
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';

// Parse without mutating the host environment; validate only export safety.
export function parseEnvironment(source) {
    if (source.includes('\0')) throw new Error('invalid environment configuration');
    const values = parseEnv(source);
    if (Object.keys(values).some(key => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key))) {
        throw new Error('invalid environment configuration');
    }
    return values;
}

const files = new Map();
function applicationEnvironment(app, inherited, configPath) {
    const home = inherited.HOME || inherited.USERPROFILE || homedir();
    const config = configPath?.startsWith('~/') ? join(home, configPath.slice(2)) : configPath;
    const path = resolve(config ? dirname(config) : join(home, '.config', app), `${app}.env`);
    if (!files.has(path)) {
        let values;
        try {
            values = parseEnvironment(readFileSync(path, 'utf8'));
        } catch (error) {
            if (error.code !== 'ENOENT') throw error;
            values = Object.create(null);
        }
        files.set(path, Object.freeze(values));
    }
    // Never mutate a shared plugin host's process.env. Empty inherited values count.
    return { ...files.get(path), ...inherited };
}

export function taskEnvironment(inherited = process.env, configPath = inherited.TASKIX_CONFIG) {
    return applicationEnvironment('taskix', inherited, configPath);
}
export function agentEnvironment(inherited = process.env, configPath = inherited.AGENTIX_CONFIG) {
    return applicationEnvironment('agentix', inherited, configPath);
}
