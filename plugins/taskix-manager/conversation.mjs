import { createHash } from "node:crypto";
import { mkdtemp, open, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

function userText(text) {
    // Hosts may put their context in user-role messages. Match complete leading
    // wrappers, not mentions of AGENTS.md in an actual request.
    for (;;) {
        const value = text.trimStart();
        let block = value;
        if (value.startsWith("# AGENTS.md instructions\n")) {
            block = value.slice(value.indexOf("\n") + 1).trimStart();
            if (!block.startsWith("<INSTRUCTIONS>")) return text;
        }
        const tag = ["INSTRUCTIONS", "environment_context", "system-reminder", "turn_aborted"]
            .find(tag => block.startsWith(`<${tag}>`));
        if (!tag || (tag === "INSTRUCTIONS" && block === value)) return text;
        const end = block.indexOf(`</${tag}>`);
        if (end < 0) return text;
        text = block.slice(end + tag.length + 3).trimStart();
    }
}

export function visibleMessage(message, identity = "") {
    if (!message || !["user", "assistant"].includes(message.role)) return undefined;
    if (message.recipient && message.recipient !== "all") return undefined;
    if (["analysis", "summary"].includes(message.channel || message.phase)) return undefined;
    let text = typeof message.content === "string" ? message.content :
        (message.content || []).filter(part => ["text", "input_text", "output_text"].includes(part.type))
            .map(part => part.text || "").join("\n");
    if (message.role === "user") text = userText(text);
    if (!text.trim()) return undefined;
    const id = message.id || createHash("sha256").update(JSON.stringify([identity, message.timestamp, message.role, text])).digest("hex");
    return { id, role: message.role, text };
}

async function* reverseLines(path) {
    const file = await open(path, "r");
    try {
        let position = (await file.stat()).size;
        let fragments = [];
        while (position > 0) {
            const length = Math.min(position, 64 * 1024);
            position -= length;
            const buffer = Buffer.allocUnsafe(length);
            let filled = 0;
            while (filled < length) {
                const {bytesRead} = await file.read(buffer, filled, length - filled, position + filled);
                if (!bytesRead) break;
                filled += bytesRead;
            }
            let end = filled;
            for (let index = filled - 1; index >= 0; index--) {
                if (buffer[index] !== 10) continue;
                fragments.push(buffer.subarray(index + 1, end));
                // Decode complete lines so chunk boundaries cannot split UTF-8.
                yield Buffer.concat(fragments.reverse()).toString("utf8");
                fragments = [];
                end = index;
            }
            fragments.push(buffer.subarray(0, end));
        }
        if (fragments.length) yield Buffer.concat(fragments.reverse()).toString("utf8");
    } finally {
        await file.close();
    }
}

export async function transcriptMessages(path) {
    const rows = [];
    let turn = "", needsTurn = false, foundUser = false;
    for await (const line of reverseLines(path)) {
        if (!line.trim()) continue;
        let row;
        try { row = JSON.parse(line); } catch { continue; } // An in-flight trailing record may be incomplete.
        const boundary = row.type === "event_msg" && row.payload?.type === "task_started";
        if (boundary) turn = row.payload.turn_id || row.timestamp;
        const codex = row.type === "response_item" && row.payload?.type === "message";
        const raw = codex ? row.payload : row.message;
        const message = !foundUser && !row.isMeta && visibleMessage(raw);
        if (message) {
            rows.push({row, raw, codex});
            needsTurn ||= (codex && !!raw.id) || (!raw.id && !row.uuid);
            // UUID-backed Claude records need no earlier turn context. For
            // fallback IDs, keep looking for a preceding task_started marker.
            if (!codex && message.role === "user") foundUser = true;
        }
        if (boundary || (foundUser && !needsTurn)) break;
    }
    return rows.reverse().map(({row, raw, codex}) => {
        const message = visibleMessage(raw, row.uuid || `${turn}:${row.timestamp}`);
        if (codex && raw.id && turn) message.id = `${turn}:${raw.id}`;
        if (!codex && row.uuid) message.id = row.uuid;
        return message;
    });
}

export async function recordMessages(messages, runner, options) {
    if (!messages.length) return;
    const directory = await mkdtemp(join(tmpdir(), "taskix-conversation-"));
    try {
        const path = join(directory, "messages.json");
        await writeFile(path, JSON.stringify(messages), {mode:0o600});
        const response = await runner(["hook", "record", "--file", path], options);
        if (response.projection_pending) throw new Error(`Conversation saved; document synchronization is pending: ${response.projection_pending}`);
    } finally {
        await rm(directory, {recursive:true, force:true});
    }
}
