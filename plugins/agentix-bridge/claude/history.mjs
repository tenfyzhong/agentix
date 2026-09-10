import { openSync, fstatSync, readSync, closeSync } from 'node:fs';
const textOf = content => typeof content === 'string' ? content : (content ?? []).filter(p => p.type === 'text').map(p => p.text).join('\n');
/** Read a bounded tail; a partial final JSONL record is left for the next read. */
export function readTranscript(path, sessionId) {
    let fd;
    try { fd = openSync(path, 'r'); } catch (error) { if (error.code === 'ENOENT') return []; throw error; }
    let text;
    try {
        const size = fstatSync(fd).size, start = Math.max(0, size - 16 * 1024 * 1024), buffer = Buffer.alloc(size - start);
        const count = readSync(fd, buffer, 0, buffer.length, start);
        text = buffer.subarray(0, count).toString('utf8');
        if (start) text = text.slice(text.indexOf('\n') + 1);
    } finally { closeSync(fd); }
    const turns = [], seen = new Set();
    for (const line of text.slice(0, text.lastIndexOf('\n') + 1).split('\n')) {
        let entry; try { entry = JSON.parse(line); } catch { continue; }
        if (entry.sessionId !== sessionId || entry.isSidechain || entry.isMeta || !entry.uuid || seen.has(entry.uuid)) continue;
        seen.add(entry.uuid);
        const message = entry.message;
        if (!message) continue;
        const content = textOf(message.content);
        if (entry.type === 'user' && content && !content.includes('<channel ')) {
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
