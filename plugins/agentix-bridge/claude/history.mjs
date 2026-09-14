import { openSync, fstatSync, readSync, closeSync } from 'node:fs';
const textOf = content => typeof content === 'string' ? content : (content ?? []).filter(p => p.type === 'text').map(p => p.text).join('\n');
const TAIL_BYTES = 16 * 1024 * 1024;
function readTail(path) {
    let fd;
    try { fd = openSync(path, 'r'); } catch (error) { if (error.code === 'ENOENT') return { start: 0, buffer: Buffer.alloc(0) }; throw error; }
    try {
        const size = fstatSync(fd).size, start = Math.max(0, size - TAIL_BYTES), buffer = Buffer.alloc(size - start);
        const count = readSync(fd, buffer, 0, buffer.length, start);
        return { start, buffer: buffer.subarray(0, count) };
    } finally { closeSync(fd); }
}
function completeRange({ start, buffer }) {
    const begin = start ? buffer.indexOf(10) + 1 : 0;
    return { begin, end: Math.max(begin, buffer.lastIndexOf(10) + 1) };
}
/** Read a bounded tail; a partial final JSONL record is left for the next read. */
export function readTranscript(path, sessionId) {
    const tail = readTail(path), { begin, end } = completeRange(tail);
    return appendRecords(tail.buffer.subarray(begin, end).toString('utf8'), sessionId, [], new Set(), false);
}
/** A session-owned latest-turn projection. Byte comparison also detects in-place rewrites. */
export function createTranscriptReader() {
    let cached;
    return (path, sessionId) => {
        const tail = readTail(path), { begin, end } = completeRange(tail);
        const appended = cached?.path === path && cached.sessionId === sessionId && cached.start === tail.start
            && cached.buffer.length <= tail.buffer.length
            && tail.buffer.subarray(0, cached.buffer.length).equals(cached.buffer);
        const state = appended ? cached : { path, sessionId, turns: [], seen: new Set(), end: begin };
        const offset = Math.max(begin, state.end);
        // Projection can fail on an invalid message shape after applying earlier rows.
        // Never retain a partially advanced projection for the next read.
        cached = undefined;
        appendRecords(tail.buffer.subarray(offset, end).toString('utf8'), sessionId, state.turns, state.seen, true);
        cached = { ...state, ...tail, end };
        return structuredClone(state.turns.at(-1));
    };
}
function appendRecords(text, sessionId, turns, seen, latestOnly) {
    for (const line of text.split('\n')) {
        if (!line) continue;
        let entry; try { entry = JSON.parse(line); } catch { continue; }
        if (entry.sessionId !== sessionId || entry.isSidechain || entry.isMeta || !entry.uuid || seen.has(entry.uuid)) continue;
        seen.add(entry.uuid);
        const message = entry.message;
        if (!message) continue;
        const content = textOf(message.content);
        if (entry.type === 'user' && content && !content.includes('<channel ')) {
            if (latestOnly) turns.length = 0;
            turns.push({ id: entry.uuid, status: 'unknown', user_text: content.slice(-100000), agent_text: '', tools: [], items: [] });
        } else if (turns.length) {
            const turn = turns.at(-1);
            if (entry.type === 'assistant') {
                if (content) turn.agent_text = [turn.agent_text, content].filter(Boolean).join('\n').slice(-100000);
                if (message.stop_reason === 'end_turn') turn.status = 'completed';
                for (const block of Array.isArray(message.content) ? message.content : []) {
                    if (block.type === 'thinking' && typeof block.thinking === 'string') turn.items.push({ id: `${entry.uuid}:reasoning:${turn.items.length}`, kind: 'reasoning', text: block.thinking.slice(0, 65536), status: 'completed' });
                    if (block.type === 'tool_use') {
                        turn.tools.push({ id: block.id, kind: block.name, label: block.name, status: 'inProgress' });
                        turn.items.push({ id: block.id, kind: block.name, text: JSON.stringify(block.input ?? {}).slice(0, 65536), status: 'inProgress' });
                    }
                }
            } else if (entry.type === 'user' && Array.isArray(message.content)) {
                for (const block of message.content.filter(b => b.type === 'tool_result')) {
                    const tool = turn.tools.find(tool => tool.id === block.tool_use_id);
                    if (tool) tool.status = block.is_error ? 'failed' : 'completed';
                    const item = turn.items.find(item => item.id === block.tool_use_id);
                    if (item) { item.status = tool.status; item.text = [item.text, textOf(block.content)].join('\n\n').slice(0, 65536); }
                }
            }
        }
    }
    return turns;
}
