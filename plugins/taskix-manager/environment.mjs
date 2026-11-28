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

// Only known filesystem settings accept home shorthand; secrets and URLs stay literal.
const pathKeys = new Set([
    'AGENTIX_CONFIG',
    'AGENTIX_LOGIN_SHELL',
    'AGENTIX_CLAUDE_DATA_DIR',
    'AGENTIX_OPENCODE_STATE_DIR',
    'AGENTIX_ANTIGRAVITY_DATA_DIR',
    'AGENTIX_ANTIGRAVITY_SESSION_DIR',
    'CODEX_HOME',
    'TASKIX_CONFIG',
    'TASKIX_BIN',
    'TASKIX_LOGIN_SHELL',
    'TASKIX_ANTIGRAVITY_DATA_DIR',
    'TASKIX_DECISION_METRICS_DB',
]);
function expandPaths(values, home) {
    return Object.fromEntries(Object.entries(values).map(([key, value]) => [
        key,
        pathKeys.has(key) && (value === '~' || value.startsWith('~/'))
            ? (value === '~' ? home : join(home, value.slice(2))) : value,
    ]));
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
            // This global setting belongs to the host, not an application file.
            delete values.XDG_STATE_HOME;
            // Application logging is configured exclusively through config.toml.
            delete values.RUST_LOG;
        } catch (error) {
            if (error.code !== 'ENOENT') throw error;
            values = Object.create(null);
        }
        files.set(path, Object.freeze(values));
    }
    // Never mutate a shared plugin host's process.env. Empty inherited values count.
    return { ...expandPaths(files.get(path), home), ...inherited };
}

export function taskEnvironment(inherited = process.env, configPath = inherited.TASKIX_CONFIG) {
    return applicationEnvironment('taskix', inherited, configPath);
}
export function agentEnvironment(inherited = process.env, configPath = inherited.AGENTIX_CONFIG) {
    return applicationEnvironment('agentix', inherited, configPath);
}
