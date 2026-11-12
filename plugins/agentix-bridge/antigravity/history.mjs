import { openSync, closeSync, fstatSync, readSync } from 'node:fs';

/** Unwrap native user input without treating ordinary inline tags as an envelope. */
export function nativeUserText(text) {
    const envelope = /^\s*<USER_REQUEST>([\s\S]*?)<\/USER_REQUEST>(?:\s*<(ADDITIONAL_METADATA|USER_SETTINGS_CHANGE)>[\s\S]*?<\/\2>)*\s*$/.exec(text);
    return envelope ? envelope[1].trim() : text;
}

const MAX_BYTES = 16 * 1024 * 1024;
const MAX_TEXT = 100000;
function tail(path, cached) {
    let fd;
    try { fd = openSync(path, 'r'); }
    catch (error) { if (error.code === 'ENOENT') return { start: 0, bytes: Buffer.alloc(0) }; throw error; }
    try {
        const stat = fstatSync(fd, { bigint: true });
        const fingerprint = [stat.dev, stat.ino, stat.size, stat.mtimeNs, stat.ctimeNs].join(':');
        if (cached?.fingerprint === fingerprint) return { unchanged: true };
        const size = Number(stat.size), start = Math.max(0, size - MAX_BYTES);
        const bytes = Buffer.alloc(size - start);
        const count = readSync(fd, bytes, 0, bytes.length, start);
        return { start, bytes: bytes.subarray(0, count), fingerprint };
    } finally { closeSync(fd); }
}

/** Project only explicit user and visible model content from native trajectory steps. */
export function createTranscriptReader() {
    let cached;
    return (path, sessionId) => {
        const { start, bytes, fingerprint, unchanged } = tail(path, cached?.path === path && cached.sessionId === sessionId ? cached : null);
        if (unchanged) return structuredClone(cached.turns);
        if (cached?.path === path && cached.sessionId === sessionId && cached.start === start && cached.bytes.equals(bytes)) {
            return structuredClone(cached.turns);
        }
        const begin = start ? bytes.indexOf(10) + 1 : 0;
        const end = bytes.lastIndexOf(10) + 1;
        const steps = new Map();
        for (const line of bytes.subarray(begin, Math.max(begin, end)).toString('utf8').split('\n')) {
            if (!line.trim()) continue;
            let step;
            try { step = JSON.parse(line); } catch { continue; }
            if (!Number.isSafeInteger(step.step_index) || step.step_index < 0) continue;
            steps.set(step.step_index, step);
        }
        const turns = [];
        for (const [index, step] of [...steps].sort((a, b) => a[0] - b[0])) {
            if (step.type === 'USER_INPUT' && step.source === 'USER_EXPLICIT' && typeof step.content === 'string') {
                turns.push({ id: `${sessionId}:step:${index}`, status: 'unknown', user_text: nativeUserText(step.content).slice(-MAX_TEXT), agent_text: '', tools: [], items: [], messages: [] });
                continue;
            }
            const turn = turns.at(-1);
            if (!turn || step.source !== 'MODEL') continue;
            if (step.type === 'PLANNER_RESPONSE') {
                const usage = { id: `${sessionId}:step:${index}` };
                for (const [native, kind] of [['input_tokens', 'input'], ['output_tokens', 'output'], ['cache_read_tokens', 'cached']]) {
                    if (Number.isSafeInteger(step[native]) && step[native] >= 0) usage[kind] = step[native];
                }
                if (Object.keys(usage).length > 1) (turn.token_usage ??= []).push(usage);
            }
            if (typeof step.content === 'string' && step.type === 'PLANNER_RESPONSE') {
                if (step.content.trim()) turn.messages.push({ id: `${sessionId}:step:${index}`, text: step.content });
                turn.agent_text = [turn.agent_text, step.content].filter(Boolean).join('\n').slice(-MAX_TEXT);
            }
            for (const [i, tool] of (Array.isArray(step.tool_calls) ? step.tool_calls : []).entries()) {
                if (typeof tool.name !== 'string') continue;
                const id = `${sessionId}:step:${index}:tool:${i}`;
                turn.tools.push({ id, kind: tool.name, label: tool.name, status: 'unknown' });
                turn.items.push({ id, kind: tool.name, text: JSON.stringify(tool.args ?? {}).slice(0, 65536), status: 'unknown' });
            }
            turn.tools = turn.tools.slice(-20);
            turn.items = turn.items.slice(-20);
        }
        cached = { path, sessionId, start, bytes, fingerprint, turns: turns.slice(-200) };
        return structuredClone(cached.turns);
    };
}
