import { createHash } from "node:crypto";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
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

export async function transcriptMessages(path) {
    const source = await readFile(path, "utf8");
    let messages = [], turn = "";
    for (const line of source.split("\n")) {
        if (!line.trim()) continue;
        let row;
        try { row = JSON.parse(line); } catch { continue; } // An in-flight trailing record may be incomplete.
        if (row.type === "event_msg" && row.payload?.type === "task_started") {
            messages = []; turn = row.payload.turn_id || row.timestamp;
        }
        if (row.isMeta) continue;
        const codex = row.type === "response_item" && row.payload?.type === "message";
        const raw = codex ? row.payload : row.message;
        const message = visibleMessage(raw, row.uuid || `${turn}:${row.timestamp}`);
        if (!message) continue;
        if (codex && raw.id && turn) message.id = `${turn}:${raw.id}`;
        if (!codex && row.uuid) message.id = row.uuid;
        // Claude transcripts have no task_started records. Tool-result messages
        // carry no visible user text and never start a new prompt here.
        if (!codex && message.role === "user") messages = [];
        messages.push(message);
    }
    return messages;
}

export async function recordMessages(messages, runner, options) {
    if (!messages.length) return;
    const directory = await mkdtemp(join(tmpdir(), "taskcli-conversation-"));
    try {
        const path = join(directory, "messages.json");
        await writeFile(path, JSON.stringify(messages), {mode:0o600});
        const response = await runner(["hook", "record", "--file", path], options);
        if (response.projection_pending) throw new Error(`Conversation saved; document synchronization is pending: ${response.projection_pending}`);
    } finally {
        await rm(directory, {recursive:true, force:true});
    }
}
